/**
 * Task dispatch: bind a queued task to its own OpenCode CEO session.
 *
 * Mirrors the managed-agent pattern in bots/agents.ts (SDK session first,
 * then registry rows), with these task-specific rules:
 *
 * - The task must be `queued`; anything else is a no-op. If a CEO link
 *   already exists (daemon restarted mid-dispatch), return it — at most one
 *   CEO per task, enforced by idx_task_agents_ceo.
 * - Per-project active-CEO cap (default 4; KBOTS_CEO_CAP overrides). Over
 *   the cap the task stays `queued` and a `task.deferred` event is logged;
 *   reconcile/next action retries it.
 * - Failure before the session exists: task stays `queued`, event
 *   `task.dispatch_failed`. Failure after the session exists: task moves to
 *   `failed`, event `task.dispatch_failed`.
 * - First prompt (title + goal) is enqueued as a `system` task_messages row
 *   and delivered immediately if the CEO is idle; otherwise it stays queued.
 * - Never throws for expected infrastructure failures — callers get a
 *   structured DispatchResult.
 *
 * Boot-time reconcileAgents() extends this module's delivery helper to
 * flush queued messages to idle CEOs.
 */
import { join } from "node:path"

import {
  addEvent,
  countActiveCeoAgents,
  createAgent,
  getDb,
  getProject,
  getTask,
  getTaskCeoAgent,
  insertTaskAgent,
  queueTaskMessage,
  transitionTaskStatus,
  updateAgent,
  type TaskRow,
  type TaskStatus,
} from "../db/repositories.ts"
import { deliverQueuedMessages } from "./delivery.ts"
import { makeOpencodeClient } from "../bots/opencode.ts"

export interface DispatchResult {
  taskId: string
  /** True when a CEO is bound to the task after this call (new or existing). */
  dispatched: boolean
  /** True when the CEO was linked before this call (idempotent no-op). */
  already?: boolean
  /** True when dispatch was skipped because the per-project CEO cap is full. */
  deferred?: boolean
  status: TaskStatus
  ceoAgentId?: string
  error?: string
}

function ceoCap(): number {
  const raw = process.env.KBOTS_CEO_CAP
  if (raw === undefined) return 4
  const parsed = Number.parseInt(raw, 10)
  return Number.isFinite(parsed) && parsed >= 1 ? parsed : 4
}

function ceoName(taskId: string): string {
  const slug = taskId.toLowerCase().replace(/[^a-z0-9_.-]+/g, "-").replace(/^-+/, "")
  return `kbots-ceo-${slug}`.slice(0, 32) || "kbots-ceo"
}

function firstPrompt(task: TaskRow): string {
  const lines = [
    `Task ${task.taskId}: ${task.title}`,
    "",
    `Goal: ${task.goal ?? task.title}`,
  ]
  if (task.detail) lines.push("", "Details:", task.detail)
  lines.push(
    "",
    "Constraints:",
    "- Work from this project directory; keep the working tree clean.",
    "- You manage coding agents and terminals; delegate implementation work.",
    "- Keep the task thread updated with progress and blockers.",
    "",
    "Start by researching the task and posting a short plan.",
  )
  return lines.join("\n")
}

function isUniqueViolation(error: unknown): boolean {
  return error instanceof Error && /UNIQUE constraint failed/i.test(error.message)
}

export async function dispatchTask(taskId: string): Promise<DispatchResult> {
  // --- safety net: never let an unexpected throw escape as a rejected promise
  let sessionCreated = false
  try {
    const task = getTask(taskId)
    if (!task) {
      return { taskId, dispatched: false, status: "queued", error: `task not found: ${taskId}` }
    }

    // Status gate + idempotency: only queued tasks dispatch, and only when
    // no CEO link exists yet.
    if (task.status !== "queued") {
      const existing = getTaskCeoAgent(taskId)
      return {
        taskId,
        dispatched: existing !== null,
        already: existing !== null,
        status: task.status,
        ...(existing ? { ceoAgentId: existing.id } : { error: `task is ${task.status}; only queued tasks dispatch` }),
      }
    }

    const failBeforeSession = (message: string): DispatchResult => {
      addEvent(
        "task.dispatch_failed",
        `Task ${taskId} dispatch failed before session creation: ${message}`,
        { taskId, phase: "preflight", message },
      )
      return { taskId, dispatched: false, status: task.status, error: message }
    }

    if (!task.projectId) return failBeforeSession("task has no project; register a project and set projectId before dispatching")
    const project = getProject(task.projectId)
    if (!project) return failBeforeSession(`unknown project: ${task.projectId}`)

    const cap = ceoCap()
    if (countActiveCeoAgents(project.projectId) >= cap) {
      addEvent(
        "task.deferred",
        `Task ${taskId} deferred: active CEO cap (${cap}) reached for project ${project.name}`,
        { taskId, projectId: project.projectId, cap },
      )
      return { taskId, dispatched: false, deferred: true, status: task.status, error: `active CEO cap (${cap}) reached; task stays queued` }
    }

    // Idempotency pre-check: a CEO linked by an earlier dispatch wins.
    const preExisting = getTaskCeoAgent(taskId)
    if (preExisting) {
      return { taskId, dispatched: true, already: true, status: task.status, ceoAgentId: preExisting.id }
    }

    const directory = join(project.path, ".agents", "bot", "ceo")

    // Registry row first (mirrors agents.start), so the session metadata
    // carries kbotsBotId. Status starting → running once committed.
    const agent = createAgent({
      projectId: project.projectId,
      role: "ceo",
      mode: "headless",
      directory,
      name: ceoName(taskId),
      status: "starting",
    })

    let sessionId: string
    try {
      const { client } = await makeOpencodeClient()
      const session = await client.session.create({
        title: `kbots CEO · ${project.name} · ${task.taskId}`,
        location: { directory },
        metadata: {
          kbotsBotId: agent.id,
          kbotsRole: "ceo",
          kbotsProjectId: project.projectId,
          kbotsTaskId: task.taskId,
        },
      })
      sessionId = session.id
      sessionCreated = true
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      updateAgent(agent.id, { status: "error", lastError: message })
      addEvent(
        "task.dispatch_failed",
        `Task ${taskId} dispatch failed before session creation: ${message}`,
        { taskId, projectId: project.projectId, agentId: agent.id, phase: "session", message },
      )
      // Failure happened before the session existed → leave the task queued.
      return { taskId, dispatched: false, status: task.status, ceoAgentId: agent.id, error: message }
    }

    // Commit the session binding + the task↔CEO link in one transaction.
    const db = getDb()
    try {
      db.transaction(() => {
        updateAgent(agent.id, { sessionId, status: "running", lastError: null }, db)
        insertTaskAgent({ taskId, agentId: agent.id, role: "ceo" }, db)
      })()
    } catch (error) {
      if (isUniqueViolation(error)) {
        // Concurrent dispatch won the idx_task_agents_ceo race: roll ours
        // back to dead and return the existing link.
        updateAgent(agent.id, { status: "error", lastError: "duplicate CEO dispatch (concurrent)" })
        const existing = getTaskCeoAgent(taskId)
        addEvent(
          "task.dispatched",
          `Task ${taskId} already has CEO ${existing?.id ?? "unknown"} (concurrent dispatch resolved)`,
          { taskId, agentId: existing?.id, already: true },
        )
        return {
          taskId,
          dispatched: existing !== null,
          already: true,
          status: task.status,
          ...(existing ? { ceoAgentId: existing.id } : { error: "duplicate CEO dispatch; link missing" }),
        }
      }
      const message = error instanceof Error ? error.message : String(error)
      // Session was created → the task fails rather than staying queued.
      transitionTaskStatus(taskId, "failed")
      addEvent(
        "task.dispatch_failed",
        `Task ${taskId} dispatch failed after session creation: ${message}`,
        { taskId, projectId: project.projectId, agentId: agent.id, sessionId, phase: "commit", message },
      )
      return { taskId, dispatched: false, status: "failed", ceoAgentId: agent.id, error: message }
    }

    // Enqueue the first prompt and deliver immediately if the CEO is idle.
    const prompt = firstPrompt(task)
    queueTaskMessage({ taskId, direction: "system", text: prompt })
    const flush = await deliverQueuedMessages(taskId)
    if (flush.delivered.length === 0) {
      addEvent(
        "task.message_queued",
        `Task ${taskId} first message queued for CEO ${agent.id}`,
        { taskId, agentId: agent.id, direction: "system", reason: flush.error ?? "busy" },
      )
    }

    transitionTaskStatus(taskId, "researching")
    addEvent(
      "task.dispatched",
      `Task ${taskId} dispatched to CEO ${agent.id} (${project.name})`,
      { taskId, agentId: agent.id, projectId: project.projectId, sessionId, mode: "headless" },
    )
    return { taskId, dispatched: true, status: "researching", ceoAgentId: agent.id }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    addEvent(
      "task.dispatch_failed",
      `Task ${taskId} dispatch failed: ${message}`,
      { taskId, phase: sessionCreated ? "commit" : "preflight", message },
    )
    if (sessionCreated) {
      try {
        transitionTaskStatus(taskId, "failed")
      } catch {
        // best effort — the status stays as-is; the event above carries the failure
      }
      return { taskId, dispatched: false, status: "failed", error: message }
    }
    return { taskId, dispatched: false, status: "queued", error: message }
  }
}
