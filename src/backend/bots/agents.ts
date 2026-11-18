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
import { realpathSync } from "node:fs"
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
  listAgents,
  listLiveAgents,
  listProjects,
  updateAgent,
  type AgentRow,
  type AgentRole,
  type ProjectRow,
} from "../db/repositories.ts"
import { RpcError, invalidParams } from "../errors.ts"
import {
  HerdrBusyError,
  HerdrError,
  getTerminal,
  interruptTerminal,
  killTerminal,
  listLiveTerminals,
  listTerminalsByLabels,
  runTerminal,
  sendPromptIfIdle,
  type HerdrFullTerminal,
} from "../adapters/herdr.ts"
import { makeOpencodeClient } from "./opencode.ts"

const MANAGED_ROLES: readonly AgentRole[] = ["ceo", "git", "ops"]

function roleParam(raw: unknown): AgentRole {
  const value = typeof raw === "string" ? raw.trim().toLowerCase() : ""
  if ((MANAGED_ROLES as readonly string[]).includes(value)) return value as AgentRole
  throw invalidParams(`role must be one of ${MANAGED_ROLES.join(", ")}`)
}

function modeParam(raw: unknown): "headless" | "terminal" {
  if (raw === undefined || raw === null) return "terminal"
  if (raw === "headless" || raw === "terminal") return raw
  throw invalidParams("mode must be 'headless' or 'terminal'")
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
  }
}

/** Probe the runtime backing an agent; never throws. */
async function probeAgent(agent: AgentRow): Promise<Record<string, unknown> | null> {
  try {
    if (agent.mode === "terminal") {
      if (!agent.hcAddress) return { state: "missing" }
      const terminal = await getTerminal(agent.hcAddress)
      if (!terminal) return { state: "missing" }
      return {
        state: terminal.state,
        status: terminal.status?.state ?? null,
        turn: terminal.turn,
        address: terminal.address,
        reason: terminal.reason,
      }
    }
    if (!agent.sessionId) return { outcome: "missing" }
    const { client } = await makeOpencodeClient()
    const session = await client.session.get({ sessionID: agent.sessionId })
    const outcome =
      session && typeof session === "object" && "outcome" in session
        ? (session as { outcome?: unknown }).outcome
        : undefined
    return { outcome: typeof outcome === "string" ? outcome : "unknown" }
  } catch {
    return null
  }
}

async function spawnTerminal(
  agent: AgentRow,
  projectPath: string,
  projectName: string,
  sessionId: string,
): Promise<HerdrFullTerminal> {
  const base = botName(projectName, agent.role)
  const roleDir = join(projectPath, ".agents", "bot", agent.role)
  for (const name of [base, `${base}-${randomSuffix()}`]) {
    try {
      return await runTerminal({
        argv: ["opencode", "--session", sessionId],
        cwd: roleDir,
        name,
        labels: { kbotsBotId: agent.id, kbotsRole: agent.role, kbotsProjectId: agent.projectId },
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
    agent = updateAgent(resumable.id, { status: "starting", mode, lastError: null, stoppedAt: null })!
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

export async function agentsStopHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  await reconcileAgents()
  let agent = mustGetAgent(params)
  if (agent.status === "stopped") {
    return managedPayload(agent, null, undefined)
  }
  agent = updateAgent(agent.id, { status: "stopping" })!
  if (agent.mode === "terminal" && agent.hcAddress) {
    try {
      await killTerminal(agent.hcAddress)
    } catch (error) {
      if (error instanceof HerdrError && error.code === 3) {
        // already gone — fall through and mark stopped
      } else {
        const message = error instanceof Error ? error.message : String(error)
        updateAgent(agent.id, { status: "error", lastError: message })
        addEvent("bot.stop_failed", `Agent ${agent.id} stop failed: ${message}`, { agentId: agent.id, message })
        throw new RpcError(-32603, `agent stop failed: ${message}`)
      }
    }
  }
  agent = updateAgent(agent.id, { status: "stopped", stoppedAt: new Date().toISOString(), lastError: null })!
  addEvent("bot.stopped", `Agent ${agent.id} ${agent.role} stopped`, {
    agentId: agent.id,
    mode: agent.mode,
    sessionId: agent.sessionId,
  })
  return managedPayload(agent, null)
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
  if (!agent.sessionId) throw invalidParams(`agent ${agent.id} has no session yet`)
  const { client } = await makeOpencodeClient()
  try {
    await client.session.interrupt({ sessionID: agent.sessionId, resume: false })
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    throw new RpcError(-32603, `interrupt failed: ${message}`)
  }
  addEvent("bot.interrupted", `Agent ${agent.id} interrupted`, { agentId: agent.id, mode: "headless" })
  return { botId: agent.id, mode: agent.mode, outcome: "stopped" }
}

export async function agentsPromptHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  await reconcileAgents()
  const agent = mustGetAgent(params)
  if (agent.status !== "running") {
    throw invalidParams(`agent ${agent.id} is ${agent.status}; prompts need a running agent`)
  }
  const text = textParam(params.text)

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
      if (error instanceof HerdrBusyError) {
        throw new RpcError(-32602, `agent ${agent.id} is busy or blocked; prompt refused (no force)`)
      }
      const message = error instanceof Error ? error.message : String(error)
      throw new RpcError(-32603, `prompt failed: ${message}`)
    }
  }

  if (!agent.sessionId) throw invalidParams(`agent ${agent.id} has no session yet`)
  const { client } = await makeOpencodeClient()
  let session: unknown
  try {
    session = await client.session.get({ sessionID: agent.sessionId })
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    throw new RpcError(-32603, `session unreachable: ${message}`)
  }
  const outcome =
    session && typeof session === "object" && "outcome" in session
      ? String((session as { outcome?: unknown }).outcome ?? "")
      : ""
  if (/blocked|permission|question/i.test(outcome)) {
    throw invalidParams(`agent ${agent.id} session is ${outcome}; refusing to prompt until it is idle`)
  }
  try {
    await client.session.prompt({ sessionID: agent.sessionId, text })
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    throw new RpcError(-32603, `prompt failed: ${message}`)
  }
  addEvent("bot.prompted", `Agent ${agent.id} prompted (${text.length} chars)`, { agentId: agent.id, textLength: text.length })
  return { botId: agent.id, mode: agent.mode, accepted: true }
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
    try {
      if (agent.mode === "terminal") {
        if (!agent.hcAddress) {
          updateAgent(agent.id, { status: "error", lastError: "spawn incomplete: no terminal address" })
          addEvent("bot.reconcile", `Agent ${agent.id} has no terminal (crash during spawn?)`, { agentId: agent.id })
          changed += 1
          continue
        }
        const terminal = await getTerminal(agent.hcAddress)
        if (!terminal) {
          updateAgent(agent.id, { status: "stopped", stoppedAt: new Date().toISOString(), lastError: "terminal missing" })
          addEvent("bot.exited", `Agent ${agent.id} terminal missing`, { agentId: agent.id })
          changed += 1
          continue
        }
        if (terminal.state === "finished") {
          const failed = (terminal.exitCode ?? 0) !== 0
          updateAgent(agent.id, {
            status: failed ? "error" : "stopped",
            stoppedAt: new Date().toISOString(),
            lastError: failed ? `terminal finished (exit ${terminal.exitCode ?? terminal.reason ?? "unknown"})` : null,
          })
          addEvent(failed ? "bot.crashed" : "bot.exited", `Agent ${agent.id} terminal finished`, {
            agentId: agent.id,
            exitCode: terminal.exitCode,
            reason: terminal.reason,
          })
          changed += 1
          continue
        }
        if (agent.status !== "running") {
          updateAgent(agent.id, { status: "running" })
          changed += 1
        }
      } else {
        if (!agent.sessionId) {
          updateAgent(agent.id, { status: "error", lastError: "headless agent has no session" })
          changed += 1
          continue
        }
        const { client } = await makeOpencodeClient()
        try {
          await client.session.get({ sessionID: agent.sessionId })
          if (agent.status !== "running") updateAgent(agent.id, { status: "running" })
        } catch {
          updateAgent(agent.id, { status: "error", lastError: "session unreachable" })
          addEvent("bot.reconcile", `Agent ${agent.id} session unreachable`, { agentId: agent.id })
          changed += 1
        }
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      updateAgent(agent.id, { status: "error", lastError: message })
      addEvent("bot.reconcile_failed", `Agent ${agent.id} reconcile failed: ${message}`, { agentId: agent.id, message })
      changed += 1
    }
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

  return {
    items,
    liveTerminals,
    filteredOut,
    managed,
    registeredProjects: allProjects.length,
    generatedAt: new Date().toISOString(),
    ...(scope ? { filteredBy: scope.projectId } : {}),
    ...(herdrUnavailable ? { herdrUnavailable } : {}),
  }
}
