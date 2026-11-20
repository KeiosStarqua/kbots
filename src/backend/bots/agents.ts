/**
 * Managed agents: start/stop/prompt/interrupt, status probing, and the
 * agents.list handler. Lifecycle mutations go through this module only —
 * the GUI, TUI and CEO bot reach them via daemon RPC.
 *
 * Partial failure is handled, not hidden, per the phase-1 policy: sessions
 * are created before terminals are spawned, rows move starting → running or
 * → error, strays are killed by label on failure when possible, and
 * reconcileAgents() re-binds the registry to reality at boot and before
 * critical operations.
 */
import { createHash } from "node:crypto"
import { existsSync, realpathSync } from "node:fs"
import { readFile, readdir } from "node:fs/promises"
import { homedir } from "node:os"
import { isAbsolute, join } from "node:path"

import {
  addEvent,
  createAgent,
  getActiveAgentByProjectRole,
  getAgent,
  getLatestAgentByProjectRole,
  getProject,
  getProjectByPath,
  getTask,
  getTaskCeoAgent,
  getTaskIdForAgent,
  listAgents,
  listLiveAgents,
  listProjects,
  listQueuedMessagesForTask,
  listTasksByStatus,
  listTasksWithQueuedMessages,
  markMessageDelivered,
  incrementMessageAttempts,
  abandonQueuedMessage,
  MESSAGE_ATTEMPT_LIMIT,
  updateAgent,
  type AgentRow,
  type AgentRole,
  type ProjectRow,
} from "../db/repositories.ts"
import { RpcError, invalidParams } from "../errors.ts"
import {
  HerdrBusyError,
  HerdrError,
  HerdrModelUnavailableError,
  getTerminal,
  readTerminal,
  interruptTerminal,
  killTerminal,
  listLiveTerminals,
  listTerminalsByLabels,
  runTerminal,
  sendPromptIfIdle,
  type HerdrFullTerminal,
} from "../adapters/herdr.ts"
import {
  availableModelKeys,
  makeOpencodeClient,
  pickFallbackModel,
  sessionModelRef,
  switchSessionModel,
  type ModelRef,
} from "./opencode.ts"
import { describeBot, isProblem, type BotReport } from "./health.ts"

// Last known model health per agent: the session's pinned model, whether the
// installed OpenCode still lists it, and the fallback it was switched to.
interface AgentModelState {
  at: number
  model: ModelRef | null
  available: boolean | null
  switchedTo: ModelRef | null
}
const MODEL_STATE_TTL_MS = 60_000
const modelStateByAgent = new Map<string, AgentModelState>()

/**
 * Keep a session usable when the model it was pinned to disappears. The model
 * list is authoritative; when the pinned model is gone, log it once and switch
 * an idle terminal agent to `KBOTS_MODEL_FALLBACK` (or another active model,
 * same provider first). Never switches mid-turn.
 */
async function ensureAgentModel(
  agent: AgentRow,
  live: { state: string | null; status: string | null },
): Promise<void> {
  if (!agent.sessionId) return
  const cached = modelStateByAgent.get(agent.id)
  if (cached && Date.now() - cached.at < MODEL_STATE_TTL_MS) return
  const model = await sessionModelRef(agent.sessionId)
  if (!model) {
    modelStateByAgent.set(agent.id, { at: Date.now(), model: null, available: null, switchedTo: null })
    return
  }
  const keys = await availableModelKeys(agent.directory)
  if (!keys) {
    // The model list itself is unreadable — say nothing, try again later.
    modelStateByAgent.set(agent.id, { at: Date.now(), model, available: null, switchedTo: null })
    return
  }
  const key = `${model.providerID}/${model.id}`
  if (keys.has(key)) {
    modelStateByAgent.set(agent.id, { at: Date.now(), model, available: true, switchedTo: null })
    return
  }
  const idle = live.status === "idle" && live.state === "live"
  const fallback = pickFallbackModel(keys, model)
  if (idle && fallback && agent.status === "running") {
    if (await switchSessionModel(agent.sessionId, fallback)) {
      modelStateByAgent.set(agent.id, { at: Date.now(), model: fallback, available: true, switchedTo: fallback })
      addEvent(
        "agent.model_switched",
        `Agent ${agent.id} model ${key} is unavailable; switched the session to ${fallback.providerID}/${fallback.id}`,
        { agentId: agent.id, from: key, to: `${fallback.providerID}/${fallback.id}` },
      )
      return
    }
  }
  if (cached?.available !== false) {
    addEvent(
      "agent.model_unavailable",
      `Agent ${agent.id} session model ${key} is unavailable${idle ? "" : " (waiting for an idle turn to switch)"}`,
      { agentId: agent.id, model: key, ...(fallback ? { fallback: `${fallback.providerID}/${fallback.id}` } : {}) },
    )
  }
  modelStateByAgent.set(agent.id, { at: Date.now(), model, available: false, switchedTo: null })
}

// Per-task cooldown for retrying dispatch of deferred (cap-blocked) tasks.
// reconcile runs at boot and before every mutation — and `list` runs every
// TUI poll — so unthrottled retries would spam `task.deferred` events.
const DEFERRED_RETRY_COOLDOWN_MS = 10 * 60 * 1000
const deferredAttemptAt = new Map<string, number>()

const MANAGED_ROLES: readonly AgentRole[] = ["ceo", "git", "ops"]

function roleParam(raw: unknown): AgentRole {
  const value = typeof raw === "string" ? raw.trim().toLowerCase() : ""
  if ((MANAGED_ROLES as readonly string[]).includes(value)) return value as AgentRole
  throw invalidParams(`role must be one of ${MANAGED_ROLES.join(", ")}`)
}

function modeParam(raw: unknown): "terminal" {
  if (raw === undefined || raw === null || raw === "terminal") return "terminal"
  throw invalidParams("headless mode has been removed; agents are spawned as hc terminals only")
}

function textParam(raw: unknown, maxLength = 8_000): string {
  const value = typeof raw === "string" ? raw.trim() : ""
  if (!value) throw invalidParams("text must be a non-empty string")
  if (value.length > maxLength) throw invalidParams(`text must be at most ${maxLength} characters`)
  return value
}

function botIdParam(raw: unknown): string {
  const value = typeof raw === "string" ? raw.trim() : ""
  if (!value) throw invalidParams("botId is required")
  return value
}

function resolveProject(params: Record<string, unknown>) {
  const ref = typeof params.project === "string" ? params.project.trim() : ""
  if (ref) {
    const project = getProject(ref)
    if (!project) throw invalidParams(`unknown project: ${ref}`)
    return project
  }
  let cwd = process.cwd()
  try {
    cwd = realpathSync(cwd)
  } catch {
    // keep literal cwd
  }
  const project = getProjectByPath(cwd)
  if (!project) {
    throw invalidParams("no project in the current directory; pass project or run inside a registered project")
  }
  return project
}

function mustGetAgent(params: Record<string, unknown>): AgentRow {
  const id = botIdParam(params.botId)
  const agent = getAgent(id)
  if (!agent) throw new RpcError(-32602, `agent not found: ${id}`)
  return agent
}

function botName(projectName: string, role: AgentRole): string {
  const slug =
    projectName
      .toLowerCase()
      .replace(/[^a-z0-9_.-]+/g, "-")
      .replace(/^[-.]+/, "")
      .slice(0, 18) || "project"
  return `kbots-${slug}-${role}`.slice(0, 32)
}

function randomSuffix(): string {
  return Math.random().toString(36).slice(2, 6)
}

function managedPayload(
  agent: AgentRow,
  live: Record<string, unknown> | null,
  projectName?: string,
): Record<string, unknown> {
  return {
    id: agent.id,
    projectId: agent.projectId,
    ...(projectName ? { projectName } : {}),
    role: agent.role,
    mode: agent.mode,
    directory: agent.directory,
    status: agent.status,
    sessionId: agent.sessionId,
    hcAddress: agent.hcAddress,
    name: agent.name,
    createdAt: agent.createdAt,
    updatedAt: agent.updatedAt,
    stoppedAt: agent.stoppedAt,
    ...(agent.lastError ? { lastError: agent.lastError } : {}),
    live,
    report: reportFor(agent, live),
  }
}

/**
 * Derive the operator-facing report for an agent (`bots/health.ts`): activity,
 * health, and the task it is bound to.
 */
export function reportFor(
  agent: AgentRow,
  live: Record<string, unknown> | null | undefined,
): BotReport {
  const taskId = getTaskIdForAgent(agent.id)
  return describeBot(agent, live, taskId ? getTask(taskId) : null)
}

/** Probe the runtime backing an agent; never throws. */
export async function probeAgent(agent: AgentRow): Promise<Record<string, unknown> | null> {
  try {
    if (agent.mode === "terminal") {
      if (!agent.hcAddress) return { state: "missing" }
      const terminal = await getTerminal(agent.hcAddress)
      if (!terminal) return { state: "missing" }
      const modelState = modelStateByAgent.get(agent.id)
      return {
        state: terminal.state,
        status: terminal.status?.state ?? null,
        turn: terminal.turn,
        address: terminal.address,
        reason: terminal.reason,
        ...(modelState
          ? {
              model: modelState.switchedTo ?? modelState.model,
              modelAvailable: modelState.available,
            }
          : {}),
      }
    }
    // Legacy headless rows (retired mode): report the last recorded outcome,
    // never call OpenCode — those agents are swept to `stopped` at reconcile.
    return { outcome: agent.lastOutcome ?? "unknown", legacy: true }
  } catch {
    return null
  }
}

/**
 * Resolve the opencode program for spawned terminals. Herdr's core resolves
 * argv[0] in its own environment, which does not inherit the daemon's PATH,
 * so prefer an absolute path when one exists.
 */
function opencodeBin(): string {
  if (process.env.KBOTS_OPENCODE_BIN) return process.env.KBOTS_OPENCODE_BIN
  const absolute = join(homedir(), ".opencode", "bin", "opencode")
  if (existsSync(absolute)) return absolute
  return "opencode"
}

export async function spawnTerminal(
  agent: AgentRow,
  projectPath: string,
  projectName: string,
  sessionId: string,
  opts?: { name?: string; extraLabels?: Record<string, string> },
): Promise<HerdrFullTerminal> {
  const base = opts?.name ?? botName(projectName, agent.role)
  const roleDir = join(projectPath, ".agents", "bot", agent.role)
  for (const name of [base, `${base}-${randomSuffix()}`]) {
    try {
      return await runTerminal({
        argv: [opencodeBin(), "--session", sessionId],
        cwd: roleDir,
        name,
        labels: {
          kbotsBotId: agent.id,
          kbotsRole: agent.role,
          kbotsProjectId: agent.projectId,
          ...opts?.extraLabels,
        },
      })
    } catch (error) {
      if (error instanceof HerdrError && /name_taken/i.test(error.message)) continue
      throw error
    }
  }
  throw new HerdrError(null, "unable to allocate a unique terminal name")
}

export async function agentsStartHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  const role = roleParam(params.role)
  const mode = modeParam(params.mode)
  const project = resolveProject(params)
  const fresh = params.fresh === true
  const directory = join(project.path, ".agents", "bot", role)

  await reconcileAgents()

  const active = getActiveAgentByProjectRole(project.projectId, role)
  if (active) {
    if (fresh) {
      throw invalidParams(`${active.id} is ${active.status} for ${project.name}/${role}; stop it first`)
    }
    return managedPayload(active, await probeAgent(active), project.name)
  }

  const latest = getLatestAgentByProjectRole(project.projectId, role)
  const resumable = latest && !fresh ? latest : null

  let agent: AgentRow
  if (resumable) {
    agent = updateAgent(resumable.id, {
      status: "starting",
      mode,
      lastError: null,
      stoppedAt: null,
      stopReason: null,
    })!
  } else {
    agent = createAgent({
      projectId: project.projectId,
      role,
      mode,
      directory,
      name: botName(project.name, role),
      status: "starting",
    })
  }

  try {
    const { client } = await makeOpencodeClient()
    let sessionId = resumable?.sessionId ?? null
    if (sessionId) {
      try {
        await client.session.get({ sessionID: sessionId })
      } catch {
        sessionId = null
      }
    }
    if (!sessionId) {
      const session = await client.session.create({
        title: `kbots ${role.toUpperCase()} · ${project.name}`,
        location: { directory },
        metadata: { kbotsBotId: agent.id, kbotsRole: role, kbotsProjectId: project.projectId },
      })
      sessionId = session.id
      agent = updateAgent(agent.id, { sessionId })!
    }

    if (mode === "terminal") {
      const spawned = await spawnTerminal(agent, project.path, project.name, sessionId)
      agent = updateAgent(agent.id, { hcAddress: spawned.address, name: spawned.name ?? agent.name, status: "running", lastError: null })!
    } else {
      agent = updateAgent(agent.id, { status: "running", lastError: null })!
    }
    addEvent("bot.started", `Agent ${agent.id} ${role} started (${mode})`, {
      agentId: agent.id,
      role,
      mode,
      sessionId: agent.sessionId,
      hcAddress: agent.hcAddress,
    })
    return managedPayload(agent, await probeAgent(agent), project.name)
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    // Compensation: kill any stray terminal labelled with this agent's id.
    try {
      const strays = await listTerminalsByLabels({ kbotsBotId: agent.id })
      for (const stray of strays) {
        if (stray.state === "live") await killTerminal(stray.address).catch(() => {})
      }
    } catch {
      // best effort
    }
    updateAgent(agent.id, { status: "error", lastError: message })
    addEvent("bot.start_failed", `Agent ${agent.id} ${role} start failed: ${message}`, { agentId: agent.id, message })
    throw new RpcError(-32603, `agent start failed: ${message}`)
  }
}

/**
 * Resume an agent against its existing OpenCode session. Used by the
 * maintenance loop when a CEO terminal was lost while its task was still open:
 * the session (and its history) is reused, never recreated. A session the
 * service no longer has throws, so the caller records the failure.
 */
export async function resumeAgentCore(agent: AgentRow, project: ProjectRow): Promise<AgentRow> {
  const sessionId = agent.sessionId
  if (!sessionId) throw new Error(`agent ${agent.id} has no session to resume`)
  let current = updateAgent(agent.id, {
    status: "starting",
    lastError: null,
    stoppedAt: null,
    stopReason: null,
  })!
  try {
    const { client } = await makeOpencodeClient()
    await client.session.get({ sessionID: sessionId })
    const spawned = await spawnTerminal(current, project.path, project.name, sessionId)
    current = updateAgent(current.id, {
      hcAddress: spawned.address,
      name: spawned.name ?? current.name,
      status: "running",
      lastError: null,
    })!
    addEvent("bot.resumed", `Agent ${current.id} ${current.role} resumed after a lost terminal`, {
      agentId: current.id,
      role: current.role,
      sessionId,
      hcAddress: current.hcAddress,
    })
    return current
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    updateAgent(current.id, { status: "error", lastError: message })
    addEvent("bot.resume_failed", `Agent ${current.id} resume failed: ${message}`, {
      agentId: current.id,
      message,
    })
    throw new Error(message)
  }
}

/**
 * Stop one managed agent: kill its terminal (when it has one), mark the row
 * stopped, and log the event. The OpenCode session is never deleted — a
 * stopped agent can be restarted against the same session.
 *
 * Shared by `agents.stop` (reason `operator`) and the daemon maintenance
 * reaper (`reaper`). `operator` is the marker dead-CEO recovery checks before
 * resuming anything: a stop the user asked for is never overridden. When the
 * terminal kill fails the row is marked `error` and the failure is rethrown.
 */
export async function stopAgentCore(
  agent: AgentRow,
  reason: "operator" | "reaper" | "reconcile",
): Promise<AgentRow> {
  if (agent.status === "stopped") return agent
  let current = updateAgent(agent.id, { status: "stopping" })!
  if (current.mode === "terminal" && current.hcAddress) {
    try {
      await killTerminal(current.hcAddress)
    } catch (error) {
      if (!(error instanceof HerdrError && error.code === 3)) {
        // code 3 = already gone — fall through and mark stopped
        const message = error instanceof Error ? error.message : String(error)
        updateAgent(current.id, { status: "error", lastError: message })
        addEvent("bot.stop_failed", `Agent ${current.id} stop failed: ${message}`, { agentId: current.id, message })
        throw error instanceof Error ? error : new Error(message)
      }
    }
  }
  current = updateAgent(current.id, {
    status: "stopped",
    stoppedAt: new Date().toISOString(),
    lastError: null,
    stopReason: reason,
  })!
  addEvent("bot.stopped", `Agent ${current.id} ${current.role} stopped`, {
    agentId: current.id,
    mode: current.mode,
    sessionId: current.sessionId,
  })
  return current
}

export async function agentsStopHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  await reconcileAgents()
  const agent = mustGetAgent(params)
  if (agent.status === "stopped") {
    return managedPayload(agent, null, undefined)
  }
  try {
    const stopped = await stopAgentCore(agent, "operator")
    return managedPayload(stopped, null)
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    throw new RpcError(-32603, `agent stop failed: ${message}`)
  }
}

export async function agentsInterruptHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  await reconcileAgents()
  const agent = mustGetAgent(params)
  if (agent.status !== "running" && agent.status !== "starting") {
    throw invalidParams(`agent ${agent.id} is ${agent.status}; only a running agent can be interrupted`)
  }
  if (agent.mode === "terminal") {
    if (!agent.hcAddress) throw invalidParams(`agent ${agent.id} has no terminal yet`)
    let outcome: { key: string; outcome: string }
    try {
      outcome = await interruptTerminal(agent.hcAddress, 10)
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      throw new RpcError(-32603, `interrupt failed: ${message}`)
    }
    addEvent("bot.interrupted", `Agent ${agent.id} interrupted (${outcome.key})`, {
      agentId: agent.id,
      ...outcome,
    })
    return { botId: agent.id, mode: agent.mode, ...outcome }
  }
  throw invalidParams("headless mode has been retired; only hc terminals can be interrupted")
}

/**
 * Shared prompt path for handlers and the watcher: status and busy guards,
 * then send. Never forces past a blocked program.
 */
export async function promptAgentCore(agent: AgentRow, text: string): Promise<Record<string, unknown>> {
  if (agent.status !== "running") {
    throw invalidParams(`agent ${agent.id} is ${agent.status}; prompts need a running agent`)
  }
  if (agent.mode === "terminal") {
    if (!agent.hcAddress) throw invalidParams(`agent ${agent.id} has no terminal yet`)
    const terminal = await getTerminal(agent.hcAddress)
    if (!terminal) {
      updateAgent(agent.id, { status: "stopped", stoppedAt: new Date().toISOString(), lastError: "terminal gone" })
      addEvent("bot.exited", `Agent ${agent.id} terminal is gone`, { agentId: agent.id })
      throw new RpcError(-32602, `agent ${agent.id} terminal is gone (marked stopped)`)
    }
    const stateWord = terminal.status?.state
    if (stateWord === "blocked" || stateWord === "error") {
      throw invalidParams(`agent ${agent.id} is ${stateWord}; refusing to prompt until it is idle`)
    }
    try {
      const result = await sendPromptIfIdle(agent.hcAddress, text)
      addEvent("bot.prompted", `Agent ${agent.id} prompted (${text.length} chars)`, { agentId: agent.id, textLength: text.length })
      return { botId: agent.id, mode: agent.mode, accepted: true, turn: result.turn }
    } catch (error) {
      if (error instanceof HerdrModelUnavailableError) {
        // A model that no longer exists is a config fix, not a busy agent: pass
        // the diagnostic (and the fix it names) through instead of "busy".
        throw new RpcError(-32602, error.message)
      }
      if (error instanceof HerdrBusyError) {
        throw new RpcError(-32602, `agent ${agent.id} is busy or blocked; prompt refused (no force)`)
      }
      const message = error instanceof Error ? error.message : String(error)
      throw new RpcError(-32603, `prompt failed: ${message}`)
    }
  }

  throw invalidParams("headless mode has been retired; only hc terminals can be prompted")
}

export async function agentsPromptHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  await reconcileAgents()
  const agent = mustGetAgent(params)
  const text = textParam(params.text)
  return promptAgentCore(agent, text)
}

/**
 * Debug surface: one agent's OpenCode session history and (for terminal
 * agents) its current screen. Read-only — never prompts, never types.
 */
export async function agentsLogsHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  const agent = mustGetAgent(params)
  const rawLimit = params.limit
  let limit = 30
  if (rawLimit !== undefined && rawLimit !== null) {
    if (typeof rawLimit !== "number" || !Number.isFinite(rawLimit)) {
      throw invalidParams("limit must be a number between 1 and 200")
    }
    limit = Math.min(200, Math.max(1, Math.trunc(rawLimit)))
  }

  const screen =
    agent.mode === "terminal" && agent.hcAddress
      ? await readTerminal(agent.hcAddress, 120).catch(() => null)
      : null

  let messages: Array<Record<string, unknown>> = []
  if (agent.sessionId) {
    try {
      const { client } = await makeOpencodeClient()
      const listed = await client.message.list({ sessionID: agent.sessionId, limit, order: "desc" })
      messages = (listed.data ?? []).map((message) => {
        const record = message as unknown as Record<string, unknown>
        const content = Array.isArray(record.content) ? (record.content as Array<Record<string, unknown>>) : []
        const text = content
          .filter((part) => part.type === "text" && typeof part.text === "string")
          .map((part) => part.text as string)
          .join("\n")
          .slice(0, 4_000)
        const time = record.time as Record<string, unknown> | undefined
        return {
          type: typeof record.type === "string" ? record.type : "unknown",
          outcome: typeof record.outcome === "string" ? record.outcome : null,
          createdAt: typeof time?.created === "number" ? new Date(time.created as number).toISOString() : null,
          text,
        }
      })
    } catch (error) {
      messages = [
        {
          type: "error",
          outcome: null,
          createdAt: null,
          text: `session history unavailable: ${error instanceof Error ? error.message : String(error)}`,
        },
      ]
    }
  }

  return {
    agentId: agent.id,
    projectId: agent.projectId,
    role: agent.role,
    mode: agent.mode,
    status: agent.status,
    sessionId: agent.sessionId,
    hcAddress: agent.hcAddress,
    live: await probeAgent(agent),
    screen,
    messages,
  }
}

export async function agentsShowHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  await reconcileAgents()
  const agent = mustGetAgent(params)
  const project = getProject(agent.projectId)
  return managedPayload(agent, await probeAgent(agent), project?.name)
}

/**
 * Rebind the registry to reality: probing every live agent, closing
 * crashed/lost rows, closing exited terminals. Best effort — failures
 * are recorded on the row, never thrown.
 */
export async function reconcileAgents(): Promise<{ checked: number; changed: number }> {
  const live = listLiveAgents()
  let changed = 0
  for (const agent of live) {
    // Task ref for CEO agents, so reconcile events carry data.taskId.
    const ceoTaskId = agent.role === "ceo" ? getTaskIdForAgent(agent.id, "ceo") : null
    try {
      if (agent.mode === "terminal") {
        if (!agent.hcAddress) {
          updateAgent(agent.id, { status: "error", lastError: "spawn incomplete: no terminal address" })
          addEvent("bot.reconcile", `Agent ${agent.id} has no terminal (crash during spawn?)`, { agentId: agent.id, ...(ceoTaskId ? { taskId: ceoTaskId } : {}) })
          changed += 1
          continue
        }
        const terminal = await getTerminal(agent.hcAddress)
        if (!terminal) {
          updateAgent(agent.id, {
            status: "stopped",
            stoppedAt: new Date().toISOString(),
            lastError: "terminal missing",
            stopReason: "terminal_missing",
          })
          addEvent("bot.exited", `Agent ${agent.id} terminal missing`, { agentId: agent.id, ...(ceoTaskId ? { taskId: ceoTaskId } : {}) })
          changed += 1
          continue
        }
        if (terminal.state === "finished") {
          const failed = (terminal.exitCode ?? 0) !== 0
          updateAgent(agent.id, {
            status: failed ? "error" : "stopped",
            stoppedAt: new Date().toISOString(),
            lastError: failed ? `terminal finished (exit ${terminal.exitCode ?? terminal.reason ?? "unknown"})` : null,
            stopReason: failed ? "crash" : "exit",
          })
          addEvent(failed ? "bot.crashed" : "bot.exited", `Agent ${agent.id} terminal finished`, {
            agentId: agent.id,
            exitCode: terminal.exitCode,
            reason: terminal.reason,
            ...(ceoTaskId ? { taskId: ceoTaskId } : {}),
          })
          changed += 1
          continue
        }
        if (agent.status !== "running") {
          updateAgent(agent.id, { status: "running" })
          changed += 1
        }
        // Keep the session usable if its pinned model disappeared.
        try {
          await ensureAgentModel(agent, {
            state: terminal.state ?? null,
            status: terminal.status?.state ?? null,
          })
        } catch {
          // Model health is advisory; reconcile must never fail on it.
        }
      } else {
        // Legacy headless rows (retired 2026-11-19): do not probe OpenCode,
        // do not spawn terminals, do not prompt. Auto-close them so old
        // kbots setups clear themselves at boot; sessions stay untouched.
        updateAgent(agent.id, {
          status: "stopped",
          stoppedAt: new Date().toISOString(),
          lastError: null,
          stopReason: "legacy_headless",
        })
        addEvent(
          "agent.legacy_headless_closed",
          `Agent ${agent.id} closed by legacy headless retirement (session kept)`,
          { agentId: agent.id, sessionId: agent.sessionId, ...(ceoTaskId ? { taskId: ceoTaskId } : {}) },
        )
        changed += 1
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      updateAgent(agent.id, { status: "error", lastError: message })
      addEvent("bot.reconcile_failed", `Agent ${agent.id} reconcile failed: ${message}`, { agentId: agent.id, message, ...(ceoTaskId ? { taskId: ceoTaskId } : {}) })
      changed += 1
    }
  }

  // v4: flush queued task messages to idle task CEOs (best effort, never
  // forces; delivery uses the same prompt path as `agents.prompt`).
  try {
    for (const task of listTasksWithQueuedMessages()) {
      const ceo = getTaskCeoAgent(task.taskId)
      if (!ceo || ceo.status !== "running") continue
      for (const message of listQueuedMessagesForTask(task.taskId)) {
        if (message.attempts >= MESSAGE_ATTEMPT_LIMIT) {
          abandonQueuedMessage(message)
          continue
        }
        const attempted = incrementMessageAttempts(message.id) ?? { ...message, attempts: message.attempts + 1 }
        try {
          await promptAgentCore(ceo, message.text)
        } catch {
          if (attempted.attempts >= MESSAGE_ATTEMPT_LIMIT) abandonQueuedMessage(attempted)
          break // busy/blocked/unreachable — keep the message queued, keep order
        }
        markMessageDelivered(message.id)
        addEvent(
          "task.message_delivered",
          `Agent ${ceo.id} received queued ${message.direction} message ${message.id}`,
          { taskId: task.taskId, agentId: ceo.id, messageId: message.id, direction: message.direction },
        )
      }
    }
  } catch (flushError) {
    // Reconcile must stay safe: log, never throw.
    console.error(`kbots-daemon: queued message flush failed: ${flushError instanceof Error ? flushError.message : String(flushError)}`)
  }

  // v5: retry dispatching cap-deferred tasks — slots may have freed because
  // a CEO finished. Rate-limited per task; dynamic import breaks the static
  // import cycle (orchestrator → delivery → this module).
  try {
    const now = Date.now()
    const queued = listTasksByStatus(["queued"]).slice().reverse()
    for (const task of queued) {
      if (now - (deferredAttemptAt.get(task.taskId) ?? 0) < DEFERRED_RETRY_COOLDOWN_MS) continue
      deferredAttemptAt.set(task.taskId, now)
      const { dispatchTask } = await import("../orchestrator/dispatch.ts")
      await dispatchTask(task.taskId)
    }
  } catch (retryError) {
    console.error(`kbots-daemon: deferred dispatch retry failed: ${retryError instanceof Error ? retryError.message : String(retryError)}`)
  }
  return { checked: live.length, changed }
}

// ---------------------------------------------------------------------------
// agents.list (extended): unmanaged items as before + managed bots merged in.
// ---------------------------------------------------------------------------

type Role = "ceo" | "git" | "ops"
type Manifest = {
  version: 1
  bots: Record<string, { enabled?: boolean }>
}
type SessionState = {
  version: 1
  projectPath: string
  sessions: Partial<Record<Role, string>>
}
type HerdrAttach = {
  address: string
  name?: string
  agent?: string
  state?: string
  status?: string
  turn?: number
}
type AgentItem = {
  projectId?: string
  projectPath: string
  projectName?: string
  role: string
  enabled: boolean | null
  sessionID: string | null
  directory: string
  registered: boolean
  manifestError?: string
  herdr?: HerdrAttach
}
type LiveTerminal = import("../adapters/herdr.ts").HerdrTerminal & {
  projectId?: string
  role?: string
}

const roles: Role[] = ["ceo", "git", "ops"]
const maxStateFiles = 200

function stateRoot(): string {
  return join(process.env.XDG_STATE_HOME ?? join(homedir(), ".local/state"), "kbots/projects")
}

function sha256hex(value: string): string {
  return createHash("sha256").update(value).digest("hex")
}

function defaultBots(): Record<Role, { enabled: boolean }> {
  return { ceo: { enabled: true }, git: { enabled: true }, ops: { enabled: true } }
}

async function readManifestBots(
  projectPath: string,
): Promise<{ bots: Record<string, { enabled?: boolean }>; manifestError?: string }> {
  try {
    const path = join(projectPath, ".agents/bot/kbots.json")
    const parsed: unknown = JSON.parse(await readFile(path, "utf8"))
    if (!parsed || typeof parsed !== "object") {
      throw new Error("manifest must be an object")
    }
    const manifest = parsed as Manifest
    if (manifest.version !== 1) {
      throw new Error("manifest must be version 1")
    }
    if (!manifest.bots || typeof manifest.bots !== "object") {
      throw new Error("manifest must define bots")
    }
    return { bots: manifest.bots }
  } catch (error) {
    const manifestError = error instanceof Error ? error.message : String(error)
    return { bots: defaultBots(), manifestError }
  }
}

function sessionIDFromState(state: unknown, role: Role): string | null {
  if (!state || typeof state !== "object") return null
  const sessions = (state as SessionState).sessions
  if (!sessions || typeof sessions !== "object") return null
  const value = sessions[role]
  return typeof value === "string" ? value : null
}

export async function agentsListHandler(params: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
  await reconcileAgents()
  const scopeRef = typeof params.project === "string" && params.project.trim() ? params.project.trim() : null
  let scope: ProjectRow | null = null
  if (scopeRef) {
    scope = getProject(scopeRef)
    if (!scope) throw invalidParams(`unknown project: ${scopeRef}`)
  }
  const registry = scope ? [scope] : listProjects()
  let orphanRegistry = listProjects()
  if (scope) orphanRegistry = [] // scoped view hides unregistered/orphan state
  const hashMap = new Map<string, ProjectRow>()
  for (const project of orphanRegistry) {
    hashMap.set(sha256hex(project.path), project)
  }

  const perProject = await Promise.all(
    registry.map(async (project) => {
      const { bots, manifestError } = await readManifestBots(project.path)
      const stateFile = join(stateRoot(), `${sha256hex(project.path)}.json`)
      let state: unknown = null
      try {
        state = JSON.parse(await readFile(stateFile, "utf8"))
      } catch {
        state = null
      }
      return { project, bots, manifestError, state }
    }),
  )

  const items: AgentItem[] = []

  for (const { project, bots, manifestError, state } of perProject) {
    for (const role of roles) {
      items.push({
        projectId: project.projectId,
        projectPath: project.path,
        projectName: project.name,
        role,
        enabled: bots[role]?.enabled !== false,
        sessionID: sessionIDFromState(state, role),
        directory: join(project.path, ".agents/bot", role),
        registered: true,
        ...(manifestError ? { manifestError } : {}),
      })
    }
  }

  const root = stateRoot()
  let stateFiles: string[] = []
  if (!scope) {
    try {
      stateFiles = (await readdir(root)).filter((entry) => entry.endsWith(".json")).slice(0, maxStateFiles)
    } catch {
      stateFiles = []
    }
  }
  for (const file of stateFiles) {
    const hash = file.slice(0, -".json".length)
    if (hashMap.has(hash)) continue
    let state: SessionState
    try {
      const parsed: unknown = JSON.parse(await readFile(join(root, file), "utf8"))
      if (!parsed || typeof parsed !== "object") continue
      state = parsed as SessionState
    } catch {
      continue
    }
    if (typeof state.projectPath !== "string" || !isAbsolute(state.projectPath)) continue
    if (!state.sessions || typeof state.sessions !== "object") continue
    for (const role of roles) {
      const sessionID = state.sessions[role]
      if (typeof sessionID !== "string") continue
      items.push({
        projectPath: state.projectPath,
        role,
        enabled: null,
        sessionID,
        directory: join(state.projectPath, ".agents/bot", role),
        registered: false,
      })
    }
  }

  const roleOrder = new Map<string, number>(roles.map((role, index) => [role, index]))
  items.sort((a, b) => {
    const byPath = a.projectPath.localeCompare(b.projectPath)
    if (byPath !== 0) return byPath
    return (roleOrder.get(a.role) ?? roles.length) - (roleOrder.get(b.role) ?? roles.length)
  })

  let liveTerminals: LiveTerminal[] = []
  let filteredOut = 0
  let herdrUnavailable: string | undefined
  try {
    const terminals = await listLiveTerminals()
    const projectRoots = registry.map((project) => {
      let root = project.path
      try {
        root = realpathSync(project.path)
      } catch {
        // keep the literal path
      }
      return { id: project.projectId, path: project.path, root }
    })
    const matchingRoot = (cwd: string) => {
      let resolved = cwd
      try {
        resolved = realpathSync(cwd)
      } catch {
        // keep the literal cwd
      }
      return projectRoots.find((project) => resolved === project.root || resolved.startsWith(`${project.root}/`))
    }
    const itemByDirectory = new Map<string, AgentItem>()
    for (const item of items) {
      if (!itemByDirectory.has(item.directory)) itemByDirectory.set(item.directory, item)
    }
    const projectByPath = new Map<string, ProjectRow>(registry.map((project) => [project.path, project]))
    const kept = terminals.filter((terminal) => matchingRoot(terminal.cwd) !== undefined)
    filteredOut = terminals.length - kept.length
    liveTerminals = kept.map((terminal) => {
      const project = matchingRoot(terminal.cwd)
      const enriched: LiveTerminal = { ...terminal, ...(project ? { projectId: project.id } : {}) }
      const hit = itemByDirectory.get(terminal.cwd)
      if (hit) {
        hit.herdr = {
          address: terminal.address,
          ...(terminal.name !== undefined ? { name: terminal.name } : {}),
          ...(terminal.agent !== undefined ? { agent: terminal.agent } : {}),
          ...(terminal.state !== undefined ? { state: terminal.state } : {}),
          ...(terminal.status !== undefined ? { status: terminal.status } : {}),
          ...(terminal.turn !== undefined ? { turn: terminal.turn } : {}),
        }
        enriched.role = hit.role
        return enriched
      }
      const row = projectByPath.get(terminal.cwd)
      if (row) enriched.projectId = row.projectId
      return enriched
    })
  } catch (error) {
    herdrUnavailable = error instanceof Error ? error.message : String(error)
  }

  const managedRows = scope ? listAgents().filter((agent) => agent.projectId === scope.projectId) : listAgents()
  const allProjects = scope ? listProjects() : registry
  const managed = await Promise.all(
    managedRows.map(async (agent) => {
      const live = await probeAgent(agent)
      const project = allProjects.find((candidate) => candidate.projectId === agent.projectId)
      return managedPayload(agent, live, project?.name)
    }),
  )

  // One derived health view per bot, so the CLI, TUI, and GUI never have to
  // re-implement the rules. `problems` is the short list an operator acts on.
  const byHealth: Record<string, number> = { ok: 0, waiting: 0, warning: 0, error: 0, stopped: 0, unknown: 0 }
  const problems: Array<Record<string, unknown>> = []
  for (const payload of managed) {
    const report = payload.report as BotReport
    byHealth[report.health.level] = (byHealth[report.health.level] ?? 0) + 1
    if (!isProblem(report)) continue
    problems.push({
      agentId: payload.id,
      role: payload.role,
      ...(typeof payload.projectName === "string" ? { projectName: payload.projectName } : {}),
      status: payload.status,
      level: report.health.level,
      code: report.health.code,
      message: report.health.message,
      ...(report.health.hint ? { hint: report.health.hint } : {}),
      ...(report.task ? { taskId: report.task.taskId } : {}),
    })
  }

  return {
    items,
    liveTerminals,
    filteredOut,
    managed,
    bots: { total: managed.length, byHealth, problemCount: problems.length },
    problems,
    registeredProjects: allProjects.length,
    generatedAt: new Date().toISOString(),
    ...(scope ? { filteredBy: scope.projectId } : {}),
    ...(herdrUnavailable ? { herdrUnavailable } : {}),
  }
}
