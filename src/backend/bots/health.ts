/**
 * Bot reporting: one derived view per managed agent that answers "what is this
 * bot doing?" and "is anything wrong?".
 *
 * The report is derived, never stored: it combines the registry row (status,
 * last error, last outcome), the live terminal probe from herdr, and the task
 * the agent is bound to. Everything the operator sees in `agents.list`,
 * `agents.show`, `system.status`, the TUI and the CLI comes from here, so the
 * wording and the health rules stay in one place.
 *
 * Health levels:
 *  - `ok`       — nothing to do; the bot is working or idle with no open task.
 *  - `waiting`  — healthy, but the task waits on a human (`review`, `blocked`).
 *  - `warning`  — suspicious, and kbots has an automatic path for it (an idle
 *                 CEO with an open task is what the nudger kicks).
 *  - `error`    — the bot cannot work until something is fixed (crashed
 *                 terminal, unreadable probe, registry error).
 *  - `stopped`  — intentionally stopped; not a problem.
 *  - `unknown`  — not probed (system.status stays DB-only for cost).
 */
import {
  TERMINAL_TASK_STATUSES,
  type AgentRow,
  type TaskRow,
  type TaskStatus,
} from "../db/repositories.ts"

export type BotActivityState =
  | "working"
  | "idle"
  | "waiting"
  | "blocked"
  | "starting"
  | "stopped"
  | "unknown"

export type BotHealthLevel = "ok" | "waiting" | "warning" | "error" | "stopped" | "unknown"

export interface BotActivity {
  state: BotActivityState
  /** Short human phrase, e.g. "working · turn 7" or "idle". */
  detail: string
}

export interface BotHealth {
  level: BotHealthLevel
  /** Stable machine code (`idle_with_open_task`, `terminal_missing`, ...). */
  code: string
  /** One sentence for the operator. Empty when the bot is fine. */
  message: string
  /** What to do about it; null when there is nothing to do. */
  hint: string | null
}

export interface BotTaskRef {
  taskId: string
  title: string
  status: TaskStatus
  updatedAt: string
}

export interface BotLiveRef {
  state: string | null
  status: string | null
  turn: number | null
}

export interface BotModelRef {
  providerID: string
  id: string
  available: boolean | null
}

export interface BotReport {
  activity: BotActivity
  health: BotHealth
  task: BotTaskRef | null
  live: BotLiveRef | null
  model: BotModelRef | null
  lastOutcome: string | null
  lastOutcomeAt: string | null
}

/**
 * How old a task may be and still be recovered automatically
 * (`KBOTS_RECOVER_WINDOW_MS`, default 2 h). Shared with the maintenance loop,
 * which owns the actual recovery, so the report can say "waiting for you"
 * instead of "will be resumed" for work that is already stale.
 */
export function recoveryWindowMs(): number {
  const raw = Number.parseInt(process.env.KBOTS_RECOVER_WINDOW_MS ?? "", 10)
  return Number.isFinite(raw) && raw >= 60_000 ? raw : 2 * 60 * 60 * 1000
}

function isTaskStale(task: BotTaskRef, now: number): boolean {
  const updated = Date.parse(task.updatedAt)
  return !Number.isFinite(updated) || now - updated > recoveryWindowMs()
}

/** Task statuses where the bot is expected to make progress on its own. */
export const ACTIVE_TASK_STATUSES: readonly TaskStatus[] = [
  "researching",
  "planning",
  "delegating",
  "running",
]

function str(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null
}

function report(
  activity: BotActivity,
  health: BotHealth,
  extra: Partial<BotReport> = {},
): BotReport {
  return {
    activity,
    health,
    task: null,
    live: null,
    model: null,
    lastOutcome: null,
    lastOutcomeAt: null,
    ...extra,
  }
}

function levelFromActivity(state: BotActivityState): BotHealthLevel {
  switch (state) {
    case "blocked":
      return "warning"
    case "waiting":
      return "waiting"
    case "stopped":
      return "stopped"
    case "unknown":
    case "starting":
      return "unknown"
    default:
      return "ok"
  }
}

/**
 * Derive the operator-facing report for one agent.
 *
 * `live` is the `probeAgent()` result: `null` means the probe failed, and
 * `undefined` means it was not attempted (system.status stays DB-only).
 */
export function describeBot(
  agent: AgentRow,
  live: Record<string, unknown> | null | undefined,
  task: TaskRow | null,
): BotReport {
  const liveRef: BotLiveRef | null = live === undefined || live === null
    ? null
    : {
        state: str(live.state),
        status: str(live.status),
        turn: typeof live.turn === "number" ? live.turn : null,
      }
  const taskRef: BotTaskRef | null = task
    ? { taskId: task.taskId, title: task.title, status: task.status, updatedAt: task.updatedAt }
    : null
  const liveRecord = live ?? null
  const liveModel = liveRecord ? (liveRecord.model as Record<string, unknown> | undefined) : undefined
  const modelRef: BotModelRef | null = liveModel && typeof liveModel.providerID === "string" && typeof liveModel.id === "string"
    ? {
        providerID: liveModel.providerID,
        id: liveModel.id,
        available: typeof liveRecord?.modelAvailable === "boolean" ? liveRecord.modelAvailable : null,
      }
    : null
  const extra: Partial<BotReport> = {
    task: taskRef,
    live: liveRef,
    model: modelRef,
    lastOutcome: agent.lastOutcome,
    lastOutcomeAt: agent.lastOutcomeAt,
  }

  // Row-level state wins: a registry error or a deliberate stop describes the
  // agent better than any probe.
  if (agent.status === "error") {
    const message = agent.lastError ?? "the agent reported an error"
    return report(
      { state: "unknown", detail: "error" },
      {
        level: "error",
        code: "agent_error",
        message,
        hint: "kbots agents stop <id> then kbots agents start <role> to retry",
      },
      extra,
    )
  }
  if (agent.status === "stopped") {
    if (taskRef && taskOpenNeedsBot(taskRef)) {
      // A stop the operator asked for is theirs to undo; anything else is the
      // maintenance loop's job (it resumes non-operator stops automatically).
      if (agent.stopReason === "operator") {
        return report(
          { state: "stopped", detail: `stopped · task ${taskRef.taskId} is ${taskRef.status}` },
          {
            level: "error",
            code: "ceo_stopped_with_open_task",
            message: `the agent is stopped while task ${taskRef.taskId} is still ${taskRef.status}`,
            hint: "kbots agents start ceo --project <ref> resumes its session",
          },
          extra,
        )
      }
      if (isTaskStale(taskRef, Date.now())) {
        return report(
          { state: "stopped", detail: `stopped · task ${taskRef.taskId} is ${taskRef.status}` },
          {
            level: "warning",
            code: "ceo_stale_with_open_task",
            message: `the agent stopped while task ${taskRef.taskId} is still ${taskRef.status} (too old for automatic recovery)`,
            hint: "kbots agents start ceo --project <ref> resumes its session",
          },
          extra,
        )
      }
      return report(
        { state: "stopped", detail: `stopped · task ${taskRef.taskId} is ${taskRef.status}` },
        {
          level: "warning",
          code: "ceo_recovering",
          message: `the agent stopped while task ${taskRef.taskId} is still ${taskRef.status}`,
          hint: "the maintenance loop resumes it against the same session",
        },
        extra,
      )
    }
    return report(
      { state: "stopped", detail: "stopped" },
      { level: "stopped", code: "stopped", message: "", hint: null },
      extra,
    )
  }
  if (agent.status === "starting") {
    return report(
      { state: "starting", detail: "starting" },
      { level: "unknown", code: "starting", message: "", hint: null },
      extra,
    )
  }

  // Not probed: report only what the row and the task can say.
  if (live === undefined) {
    const openTask = taskRef ? taskOpenNeedsBot(taskRef) : false
    return report(
      { state: "unknown", detail: "not probed" },
      openTask
        ? {
            level: "warning",
            code: "idle_with_open_task",
            message: `task ${taskRef!.taskId} is ${taskRef!.status} and the bot is running`,
            hint: "the maintenance tick nudges or recovers this task",
          }
        : { level: levelFromActivity("unknown"), code: "unknown", message: "", hint: null },
      extra,
    )
  }

  if (live === null) {
    return report(
      { state: "unknown", detail: "probe failed" },
      {
        level: "warning",
        code: "probe_failed",
        message: "the terminal state could not be read",
        hint: "check herdr (`hc ls`); the maintenance tick reconciles agents",
      },
      extra,
    )
  }

  const state = str(live.state)
  const terminalStatus = str(live.status)
  const turn = typeof live.turn === "number" ? live.turn : null

  if (state === "missing") {
    return report(
      { state: "unknown", detail: "terminal missing" },
      {
        level: "error",
        code: "terminal_missing",
        message: "the terminal is gone but the agent is still registered as live",
        hint: "kbots agents stop <id> clears the row; the session is kept",
      },
      extra,
    )
  }
  if (state === "finished") {
    const reason = str(live.reason)
    const exit = typeof live.exitCode === "number" ? live.exitCode : null
    const why = exit !== null ? `exit ${exit}` : reason ?? "unknown reason"
    return report(
      { state: "unknown", detail: `terminal finished (${why})` },
      {
        level: "error",
        code: "terminal_finished",
        message: `the terminal exited: ${why}`,
        hint: "the maintenance tick reconciles this; kbots agents start <role> resumes the session",
      },
      extra,
    )
  }

  if (terminalStatus === "blocked") {
    const blockedReason = str(live.reason)
    return report(
      { state: "blocked", detail: blockedReason ? `blocked · ${blockedReason}` : "blocked" },
      {
        level: "warning",
        code: "terminal_blocked",
        message: blockedReason ?? "the program is waiting for input",
        hint: "kbots agents prompt <id> \"…\" or kbots agents interrupt <id>",
      },
      extra,
    )
  }

  if (terminalStatus === "busy") {
    return report(
      { state: "working", detail: turn !== null ? `working · turn ${turn}` : "working" },
      { level: "ok", code: "working", message: "", hint: null },
      extra,
    )
  }

  if (terminalStatus === "idle") {
    const detail = turn !== null ? `idle · turn ${turn}` : "idle"
    // A pinned model that no longer exists means this idle bot cannot start a
    // turn at all: say so before the task-based idle states, and only when idle
    // (a blocked or working terminal has a more immediate state to report).
    if (modelRef && modelRef.available === false) {
      const label = `${modelRef.providerID}/${modelRef.id}`
      return report(
        { state: "idle", detail },
        {
          level: "warning",
          code: "model_unavailable",
          message: `session model ${label} is unavailable`,
          hint: 'kbots switches to a fallback when the terminal is idle (KBOTS_MODEL_FALLBACK); pin one with "model" in .agents/bot/<role>/opencode.jsonc',
        },
        extra,
      )
    }
    if (!taskRef) {
      return report(
        { state: "idle", detail },
        { level: "ok", code: "idle", message: "", hint: null },
        extra,
      )
    }
    if (taskOpenNeedsBot(taskRef)) {
      return report(
        { state: "idle", detail },
        {
          level: "warning",
          code: "idle_with_open_task",
          message: `idle while task ${taskRef.taskId} is ${taskRef.status}`,
          hint: "the nudger kicks this task automatically (KBOTS_NUDGE)",
        },
        extra,
      )
    }
    if (TERMINAL_TASK_STATUSES.includes(taskRef.status)) {
      // Task already finished but the terminal is still up: harmless, and the
      // maintenance reaper stops it after the grace period.
      return report(
        { state: "idle", detail },
        {
          level: "ok",
          code: "idle_task_finished",
          message: "",
          hint: null,
        },
        extra,
      )
    }
    return report(
      { state: "waiting", detail: `waiting · task ${taskRef.taskId} is ${taskRef.status}` },
      { level: "waiting", code: "waiting_on_human", message: "", hint: null },
      extra,
    )
  }

  return report(
    { state: "unknown", detail: terminalStatus ? `terminal ${terminalStatus}` : "unknown" },
    { level: levelFromActivity("unknown"), code: "unknown", message: "", hint: null },
    extra,
  )
}

function taskOpenNeedsBot(task: BotTaskRef): boolean {
  return (ACTIVE_TASK_STATUSES as readonly string[]).includes(task.status)
}

/** True when the report describes something an operator should look at. */
export function isProblem(report: BotReport): boolean {
  return report.health.level === "error" || report.health.level === "warning"
}

/** Compact one-line problem for logs and status summaries. */
export function problemLine(
  report: BotReport,
  agent: Pick<AgentRow, "id" | "role">,
): string {
  const task = report.task ? ` · ${report.task.taskId}` : ""
  return `${agent.id} ${agent.role}${task}: ${report.health.message || report.health.code}`
}
