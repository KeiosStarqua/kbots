/**
 * Daemon maintenance: the periodic tick that keeps the registry honest and
 * reclaims terminals whose work is already finished.
 *
 * Three steps, in order:
 *  1. `reconcileAgents()` — the same probe `agents.*` mutations run, now also
 *     on a timer, so a terminal that exits between RPCs is noticed instead of
 *     waiting for the next client call.
 *  2. The reaper — a CEO terminal whose task already reached a terminal status
 *     (`done`, `failed`, `canceled`) for longer than the grace period is
 *     stopped. The OpenCode session and its history stay untouched, exactly as
 *     `agents.stop` leaves them; only the terminal process goes away.
 *  3. Orphan sweep — live kbots-labelled terminals whose address is no longer
 *     referenced by any agent row (left behind when a respawn overwrote
 *     `hcAddress`) are reclaimed. A terminal must appear in two consecutive
 *     ticks before it is killed, so a spawn in progress is never caught.
 *  4. Stale-task continuation — a CEO that stopped outside the crash recovery
 *     window is resumed by the same path, so an old task cannot sit in
 *     `researching` forever. It shares the recovery budget, cooldown and CEO
 *     cap; `KBOTS_STALE_CONTINUE=off` disables only this step.
 *
 * Deliberately narrow:
 *  - Only `role = ceo`, `mode = terminal`, `status = running` agents with a
 *    task link are reaped. git/ops bots are long-lived per project, and an
 *    agent without a task link is not attributable — never touched.
 *  - The grace period counts from the task's `updated_at`, so a CEO still
 *    writing its final report is not killed mid-turn.
 *  - A kill failure marks the row `error` and logs `agent.reap_failed`; the
 *    loop continues and retries the other agents on the next tick.
 *  - Crash recovery skips a task older than `KBOTS_RECOVER_WINDOW_MS`
 *    (default 2 h); the stale-task step handles those with the same bounded
 *    budget instead of leaving them open forever.
 *
 * Env:
 *  - `KBOTS_REAP=off` — skip the reaper step; the reconcile tick keeps running.
 *  - `KBOTS_ORPHAN_REAP=off` — skip the orphan sweep.
 *  - `KBOTS_REAP_INTERVAL_MS` — tick period, default 60000, minimum 10000.
 *  - `KBOTS_REAP_GRACE_MS` — quiet time after the task finished, default
 *    600000 (10 min); `0` reaps on the first tick after the task finishes.
 *  - `KBOTS_STALE_CONTINUE=off` — skip stale-task continuation; crash recovery
 *    and the reaper keep running. `KBOTS_RECOVER=off` disables both resumes.
 */
import {
  addEvent,
  appendBriefBlocker,
  countActiveCeoAgents,
  getProject,
  getTask,
  getTaskCeoAgent,
  getTaskIdForAgent,
  isLiveAgentStatus,
  listAgents,
  listLiveAgents,
  listTasksByStatus,
  recordRecovery,
  transitionTaskStatus,
  TERMINAL_TASK_STATUSES,
  type AgentRow,
  type TaskRow,
} from "../db/repositories.ts"
import {
  MANAGED_ROLES,
  promptAgentCore,
  reconcileAgents,
  resumeAgentCore,
  stopAgentCore,
} from "../bots/agents.ts"
import { ACTIVE_TASK_STATUSES, recoveryWindowMs, staleContinueEnabled } from "../bots/health.ts"
import {
  getTerminal,
  killTerminal,
  listTerminalsByLabels,
  type HerdrFullTerminal,
} from "../adapters/herdr.ts"

const DEFAULT_INTERVAL_MS = 60_000
const DEFAULT_GRACE_MS = 10 * 60 * 1000
const MIN_INTERVAL_MS = 10_000
const DEFAULT_MAX_RECOVERIES = 2
const DEFAULT_RECOVERY_COOLDOWN_MS = 5 * 60 * 1000
const DEFAULT_CEO_CAP = 4

function envMs(name: string, fallback: number, minimum: number): number {
  const raw = Number.parseInt(process.env[name] ?? "", 10)
  return Number.isFinite(raw) && raw >= minimum ? raw : fallback
}

export function reapIntervalMs(): number {
  return envMs("KBOTS_REAP_INTERVAL_MS", DEFAULT_INTERVAL_MS, MIN_INTERVAL_MS)
}

export function reapGraceMs(): number {
  return envMs("KBOTS_REAP_GRACE_MS", DEFAULT_GRACE_MS, 0)
}

export function reaperEnabled(): boolean {
  return process.env.KBOTS_REAP !== "off"
}

/**
 * Live CEO terminals whose task is already terminal and has been quiet for at
 * least the grace period. Read-only: the caller decides to stop them.
 */
export function findReapableAgents(now: number = Date.now()): AgentRow[] {
  const grace = reapGraceMs()
  const reapable: AgentRow[] = []
  for (const agent of listLiveAgents()) {
    if (agent.role !== "ceo" || agent.mode !== "terminal" || agent.status !== "running") continue
    const taskId = getTaskIdForAgent(agent.id, "ceo")
    if (!taskId) continue
    const task = getTask(taskId)
    if (!task) continue
    if (!(TERMINAL_TASK_STATUSES as readonly string[]).includes(task.status)) continue
    const finishedAt = Date.parse(task.updatedAt)
    if (!Number.isFinite(finishedAt) || now - finishedAt < grace) continue
    reapable.push(agent)
  }
  return reapable
}

/**
 * Stop every reapable CEO terminal. Returns how many were stopped. Sessions
 * are kept; only the terminal process is reclaimed.
 */
export async function reapFinishedTerminals(now: number = Date.now()): Promise<number> {
  if (!reaperEnabled()) return 0
  let stopped = 0
  for (const agent of findReapableAgents(now)) {
    const taskId = getTaskIdForAgent(agent.id, "ceo")
    try {
      await stopAgentCore(agent, "reaper")
      stopped += 1
      addEvent("agent.reaped", `Agent ${agent.id} reaped: task ${taskId ?? "?"} finished`, {
        agentId: agent.id,
        sessionId: agent.sessionId,
        ...(taskId ? { taskId } : {}),
        reason: "task_finished",
      })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      addEvent("agent.reap_failed", `Agent ${agent.id} reap failed: ${message}`, {
        agentId: agent.id,
        ...(taskId ? { taskId } : {}),
        message,
      })
    }
  }
  return stopped
}

// -- orphan terminal sweep ---------------------------------------------------

export function orphanReapEnabled(): boolean {
  return process.env.KBOTS_ORPHAN_REAP !== "off"
}

/** A live kbots terminal no agent row points at any more. */
export interface OrphanTerminal {
  address: string
  name: string | null
  labels: Record<string, string>
}

/**
 * Pure selection rule for the sweep. Only terminals carrying a kbots label
 * (`kbotsBotId` or `kbotsRole`) are ours; everything else (router terminals,
 * plain shells) is left alone even when no registry row references it.
 */
export function selectOrphanTerminals(
  terminals: ReadonlyArray<{ address: string; state: string; name?: string | null; labels: Record<string, string> }>,
  referencedAddresses: ReadonlySet<string>,
): OrphanTerminal[] {
  return terminals.filter(
    (terminal) =>
      terminal.state === "live" &&
      (Object.prototype.hasOwnProperty.call(terminal.labels, "kbotsBotId") ||
        Object.prototype.hasOwnProperty.call(terminal.labels, "kbotsRole")) &&
      !referencedAddresses.has(terminal.address),
  ).map((terminal) => ({
    address: terminal.address,
    name: terminal.name ?? null,
    labels: terminal.labels,
  }))
}

/**
 * First sighting of an unreferenced terminal only records it; a second
 * consecutive sighting kills it. That one-tick window covers the gap between
 * `spawnTerminal` and the `hcAddress` update, and any slow DB write.
 */
const pendingOrphans = new Set<string>()

export async function reapOrphanTerminals(): Promise<number> {
  if (!orphanReapEnabled()) return 0

  let terminals: HerdrFullTerminal[] = []
  try {
    for (const role of MANAGED_ROLES) {
      terminals.push(...await listTerminalsByLabels({ kbotsRole: role }))
    }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    addEvent("agent.orphan_sweep_failed", `Orphan sweep could not list terminals: ${message}`, { message })
    return 0
  }

  // Keep every address any row holds, including stopped ones: a row may be in
  // the middle of stopping and its terminal must not be swept out from under it.
  const referenced = new Set(
    listAgents()
      .map((agent) => agent.hcAddress)
      .filter((address): address is string => typeof address === "string" && address.length > 0),
  )
  const orphans = selectOrphanTerminals(terminals, referenced)
  const current = new Set(orphans.map((terminal) => terminal.address))
  for (const address of pendingOrphans) {
    if (!current.has(address)) pendingOrphans.delete(address)
  }

  let killed = 0
  for (const terminal of orphans) {
    if (!pendingOrphans.has(terminal.address)) {
      pendingOrphans.add(terminal.address)
      continue
    }
    // `hc ls` omits labels; the role filter supplies one, but the full record
    // has the bot/task ids that make the event useful to an operator.
    let labels = terminal.labels
    try {
      const full = await getTerminal(terminal.address)
      if (full) labels = { ...labels, ...full.labels }
    } catch {
      // keep the labels we already have
    }
    const { kbotsBotId: botId, kbotsRole: role, kbotsProjectId: projectId, kbotsTaskId: taskId } = labels
    try {
      await killTerminal(terminal.address)
      killed += 1
      addEvent(
        "agent.orphan_reaped",
        `Agent ${botId ?? "?"} orphan terminal ${terminal.address} reaped`,
        {
          address: terminal.address,
          ...(terminal.name ? { name: terminal.name } : {}),
          ...(botId ? { agentId: botId } : {}),
          ...(role ? { role } : {}),
          ...(projectId ? { projectId } : {}),
          ...(taskId ? { taskId } : {}),
          reason: "unreferenced_terminal",
        },
      )
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      addEvent(
        "agent.orphan_reap_failed",
        `Orphan terminal ${terminal.address} could not be reaped: ${message}`,
        { address: terminal.address, ...(botId ? { agentId: botId } : {}), message },
      )
    }
    pendingOrphans.delete(terminal.address)
  }
  return killed
}

export async function tick(): Promise<void> {
  // Reconcile may stop rows as it discovers dead terminals, so it runs first;
  // the reaper then only sees agents that really are still running, and the
  // recovery step sees the rows that just died. The orphan sweep runs after
  // the reaper so a terminal just retired by it is not mistaken for a stray.
  await reconcileAgents()
  await reapFinishedTerminals()
  await reapOrphanTerminals()
  await recoverLostTaskCeos()
  await continueStaleTaskCeos()
}

/** Pure helpers exposed for the stand-alone self-check; not daemon API. */
export const __maintenanceInternals = { selectOrphanTerminals, orphanReapEnabled }

// -- dead-CEO recovery ------------------------------------------------------

export function recoveryMax(): number {
  const raw = Number.parseInt(process.env.KBOTS_RECOVER_MAX ?? "", 10)
  return Number.isFinite(raw) && raw >= 0 ? raw : DEFAULT_MAX_RECOVERIES
}

export function recoveryCooldownMs(): number {
  return envMs("KBOTS_RECOVER_COOLDOWN_MS", DEFAULT_RECOVERY_COOLDOWN_MS, 0)
}

export function recoveryEnabled(): boolean {
  return process.env.KBOTS_RECOVER !== "off"
}

function ceoCap(): number {
  const raw = Number.parseInt(process.env.KBOTS_CEO_CAP ?? "", 10)
  return Number.isFinite(raw) && raw > 0 ? raw : DEFAULT_CEO_CAP
}

function isRecoverable(agent: AgentRow): boolean {
  if (agent.status === "error") return true
  if (agent.status !== "stopped") return false
  // `operator` marks a stop the user asked for: never override it.
  return agent.stopReason !== "operator"
}

function isStale(task: TaskRow, now: number): boolean {
  const updated = Date.parse(task.updatedAt)
  return !Number.isFinite(updated) || now - updated > recoveryWindowMs()
}

/**
 * Resume a CEO whose terminal died while its task is still open.
 *
 * Only `error` rows are resumed: a row is marked `error` by reconcile when the
 * terminal failed (non-zero exit) or the probe broke, while a deliberate
 * `agents.stop` leaves `stopped` — the operator's intent is never overridden.
 * A stopped CEO with an open task shows up as `ceo_stopped_with_open_task` in
 * the bot report instead.
 *
 * Each attempt spends one unit of the per-task budget (`tasks.recoveries`),
 * so a crash loop ends with the task moved to `blocked` and a blocker note
 * rather than with an endless restart cycle. Attempts are rate-limited by
 * `KBOTS_RECOVER_COOLDOWN_MS`.
 */
export async function recoverLostTaskCeos(
  now: number = Date.now(),
): Promise<{ recovered: number; blocked: number }> {
  if (!recoveryEnabled()) return { recovered: 0, blocked: 0 }
  let recovered = 0
  let blocked = 0
  for (const task of listTasksByStatus([...ACTIVE_TASK_STATUSES])) {
    const ceo = getTaskCeoAgent(task.taskId)
    if (!ceo || isLiveAgentStatus(ceo.status)) continue
    if (!isRecoverable(ceo)) continue
    if (isStale(task, now)) continue

    if (task.recoveries >= recoveryMax()) {
      transitionTaskStatus(task.taskId, "blocked")
      appendBriefBlocker(
        task.taskId,
        `CEO terminal lost ${task.recoveries} times; recovery budget exhausted, needs a human`,
      )
      addEvent("task.ceo_unrecoverable", `Task ${task.taskId} blocked: no recovery budget left`, {
        taskId: task.taskId,
        agentId: ceo.id,
        recoveries: task.recoveries,
      })
      blocked += 1
      continue
    }

    const lastAttempt = task.lastRecoveryAt ? Date.parse(task.lastRecoveryAt) : Number.NaN
    if (Number.isFinite(lastAttempt) && now - lastAttempt < recoveryCooldownMs()) continue

    const project = task.projectId ? getProject(task.projectId) : null
    if (!project) continue
    if (countActiveCeoAgents(project.projectId) >= ceoCap()) {
      addEvent("task.ceo_recovery_deferred", `Task ${task.taskId} recovery deferred: CEO cap reached`, {
        taskId: task.taskId,
        agentId: ceo.id,
        cap: ceoCap(),
      })
      continue
    }

    // Spend the budget before attempting: a spawn that keeps failing must not
    // loop forever with a fresh counter every tick.
    recordRecovery(task.taskId, new Date(now).toISOString())
    try {
      const resumed = await resumeAgentCore(ceo, project)
      recovered += 1
      addEvent("task.ceo_recovered", `Task ${task.taskId} CEO ${ceo.id} resumed`, {
        taskId: task.taskId,
        agentId: ceo.id,
        recoveries: task.recoveries + 1,
      })
      try {
        await promptAgentCore(
          resumed,
          `Your terminal was restarted after a crash. Continue task ${task.taskId} ("${task.title}"); it is still ${task.status}. Keep the task thread updated.`,
        )
      } catch {
        // Busy or not ready yet — the nudger picks the task up on its own.
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      addEvent("task.ceo_recovery_failed", `Task ${task.taskId} recovery failed: ${message}`, {
        taskId: task.taskId,
        agentId: ceo.id,
        message,
      })
    }
  }
  return { recovered, blocked }
}

// -- stale-task continuation -------------------------------------------------

/**
 * Continue an old task whose CEO stopped outside the crash recovery window.
 *
 * `recoverLostTaskCeos` deliberately ignores tasks older than
 * `KBOTS_RECOVER_WINDOW_MS`; without a second path a task whose CEO died days
 * ago stays in `researching` forever. This is that second path: it uses the
 * same budget (`tasks.recoveries`), cooldown (`last_recovery_at`) and CEO cap,
 * resumes the CEO against its existing session and asks it to continue. When
 * the budget runs out the task is blocked with a note instead of being
 * restarted forever.
 */
export async function continueStaleTaskCeos(
  now: number = Date.now(),
): Promise<{ continued: number; blocked: number }> {
  if (!recoveryEnabled() || !staleContinueEnabled()) return { continued: 0, blocked: 0 }
  let continued = 0
  let blocked = 0
  for (const task of listTasksByStatus([...ACTIVE_TASK_STATUSES])) {
    // Fresh work belongs to the crash recovery step; only handle what that
    // step skipped for being too old.
    if (!isStale(task, now)) continue
    const ceo = getTaskCeoAgent(task.taskId)
    if (!ceo || isLiveAgentStatus(ceo.status)) continue
    if (!isRecoverable(ceo)) continue

    if (task.recoveries >= recoveryMax()) {
      transitionTaskStatus(task.taskId, "blocked")
      appendBriefBlocker(
        task.taskId,
        `Stale task CEO continued ${task.recoveries} times; still ${task.status}, needs a human`,
      )
      addEvent("task.stale_unrecoverable", `Task ${task.taskId} blocked: stale task needs a human`, {
        taskId: task.taskId,
        agentId: ceo.id,
        recoveries: task.recoveries,
      })
      blocked += 1
      continue
    }

    const lastAttempt = task.lastRecoveryAt ? Date.parse(task.lastRecoveryAt) : Number.NaN
    if (Number.isFinite(lastAttempt) && now - lastAttempt < recoveryCooldownMs()) continue

    const project = task.projectId ? getProject(task.projectId) : null
    if (!project) continue
    if (countActiveCeoAgents(project.projectId) >= ceoCap()) {
      addEvent("task.stale_continue_deferred", `Task ${task.taskId} continuation deferred: CEO cap reached`, {
        taskId: task.taskId,
        agentId: ceo.id,
        cap: ceoCap(),
      })
      continue
    }

    // Spend the budget before the spawn so a repeated failure cannot loop with
    // a fresh counter on every tick.
    recordRecovery(task.taskId, new Date(now).toISOString())
    try {
      const resumed = await resumeAgentCore(ceo, project)
      continued += 1
      const updated = Date.parse(task.updatedAt)
      addEvent("task.stale_continued", `Task ${task.taskId} CEO ${ceo.id} resumed for stale continuation`, {
        taskId: task.taskId,
        agentId: ceo.id,
        status: task.status,
        ageMs: Number.isFinite(updated) ? now - updated : null,
      })
      try {
        await promptAgentCore(
          resumed,
          `Continue stale task ${task.taskId} ("${task.title}"). It is still ${task.status}; either finish it and run \`kb tasks update ${task.taskId} --status done --note "<one-line result>"\`, or report exactly what blocks it. Do not pick up unrelated work.`,
        )
      } catch {
        // Not ready yet or busy — queued messages and the nudger pick it up.
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      addEvent("task.stale_continue_failed", `Task ${task.taskId} stale continuation failed: ${message}`, {
        taskId: task.taskId,
        agentId: ceo.id,
        message,
      })
    }
  }
  return { continued, blocked }
}

/**
 * Start the maintenance loop. The daemon already reconciles once at boot, so
 * the loop does not prime an extra pass at t0. Overlapping ticks are skipped:
 * a slow reconcile must never pile up.
 */
export function startMaintenanceLoop(): () => void {
  let busy = false
  const timer = setInterval(() => {
    if (busy) return
    busy = true
    void tick()
      .catch((error: unknown) => {
        console.error(`kbots-daemon: maintenance tick failed: ${error instanceof Error ? error.message : String(error)}`)
      })
      .finally(() => {
        busy = false
      })
  }, reapIntervalMs())
  timer.unref()
  return () => clearInterval(timer)
}
