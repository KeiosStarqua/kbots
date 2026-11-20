/**
 * Daemon maintenance: the periodic tick that keeps the registry honest and
 * reclaims terminals whose work is already finished.
 *
 * Two steps, in order:
 *  1. `reconcileAgents()` — the same probe `agents.*` mutations run, now also
 *     on a timer, so a terminal that exits between RPCs is noticed instead of
 *     waiting for the next client call.
 *  2. The reaper — a CEO terminal whose task already reached a terminal status
 *     (`done`, `failed`, `canceled`) for longer than the grace period is
 *     stopped. The OpenCode session and its history stay untouched, exactly as
 *     `agents.stop` leaves them; only the terminal process goes away.
 *
 * Deliberately narrow:
 *  - Only `role = ceo`, `mode = terminal`, `status = running` agents with a
 *    task link are reaped. git/ops bots are long-lived per project, and an
 *    agent without a task link is not attributable — never touched.
 *  - The grace period counts from the task's `updated_at`, so a CEO still
 *    writing its final report is not killed mid-turn.
 *  - A kill failure marks the row `error` and logs `agent.reap_failed`; the
 *    loop continues and retries the other agents on the next tick.
 *
 * Env:
 *  - `KBOTS_REAP=off` — skip the reaper step; the reconcile tick keeps running.
 *  - `KBOTS_REAP_INTERVAL_MS` — tick period, default 60000, minimum 10000.
 *  - `KBOTS_REAP_GRACE_MS` — quiet time after the task finished, default
 *    600000 (10 min); `0` reaps on the first tick after the task finishes.
 */
import {
  addEvent,
  getTask,
  getTaskIdForAgent,
  listLiveAgents,
  TERMINAL_TASK_STATUSES,
  type AgentRow,
} from "../db/repositories.ts"
import { reconcileAgents, stopAgentCore } from "../bots/agents.ts"

const DEFAULT_INTERVAL_MS = 60_000
const DEFAULT_GRACE_MS = 10 * 60 * 1000
const MIN_INTERVAL_MS = 10_000

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
      await stopAgentCore(agent)
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

export async function tick(): Promise<void> {
  // Reconcile may stop rows as it discovers dead terminals, so it runs first;
  // the reaper then only sees agents that really are still running.
  await reconcileAgents()
  await reapFinishedTerminals()
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
