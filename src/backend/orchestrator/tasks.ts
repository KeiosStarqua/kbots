import { RpcError, invalidParams } from "../errors.ts"
import { dispatchTask } from "./dispatch.ts"
import { deliverQueuedMessages } from "./delivery.ts"
import {
  addEvent,
  appendBriefDecision,
  cancelTask,
  countPendingApprovals,
  countTasks,
  createTask,
  getProject,
  getTask,
  getTaskBrief,
  getTaskWithLink,
  listApprovals,
  listMessagesForTask,
  listQueuedUserMessagesForInbox,
  listTaskAgents,
  listTasks,
  listTasksByStatus,
  queueTaskMessage,
  transitionTaskStatus,
  TASK_STATUSES,
  TERMINAL_TASK_STATUSES,
  type TaskRow,
  type TaskStatus,
} from "../db/repositories.ts"

function projectRef(raw: unknown): string | undefined {
  if (raw === undefined || raw === null) return undefined
  return typeof raw === "string" ? raw.trim() : String(raw)
}

function resolveProjectId(params: Record<string, unknown>, blankMeansAbsent: boolean): string | undefined {
  const ref = projectRef(params.project)
  if (ref === undefined || (blankMeansAbsent && ref === "")) return undefined
  const project = getProject(ref)
  if (!project) throw invalidParams(`unknown project: ${ref}`)
  return project.projectId
}

function requiredTaskId(raw: unknown): string {
  const taskId = typeof raw === "string" ? raw.trim() : ""
  if (!taskId) throw invalidParams("taskId is required")
  return taskId
}

function optionalText(raw: unknown, field: string, maxLength: number): string | undefined {
  if (raw === undefined || raw === null) return undefined
  if (typeof raw !== "string") throw invalidParams(`${field} must be a string`)
  const value = raw.trim()
  if (value.length > maxLength) throw invalidParams(`${field} must be at most ${maxLength} characters`)
  return value.length > 0 ? value : undefined
}

function requiredText(raw: unknown, field: string, maxLength: number): string {
  const value = optionalText(raw, field, maxLength)
  if (value === undefined) throw invalidParams(`${field} must be a non-empty string`)
  return value
}

function statusParam(raw: unknown): TaskStatus | undefined {
  if (raw === undefined || raw === null) return undefined
  if (typeof raw !== "string" || !(TASK_STATUSES as readonly string[]).includes(raw)) {
    throw invalidParams(`status must be one of ${TASK_STATUSES.join(", ")}`)
  }
  return raw as TaskStatus
}

function truncate(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text
}

function briefTaskSummary(task: TaskRow): Record<string, unknown> {
  return {
    taskId: task.taskId,
    title: task.title,
    status: task.status,
    projectId: task.projectId,
    projectPath: task.projectPath,
    goal: task.goal,
    createdAt: task.createdAt,
    updatedAt: task.updatedAt,
  }
}

function inboxItems(): Array<Record<string, unknown>> {
  const items: Array<Record<string, unknown>> = []
  for (const approval of listApprovals({ status: "pending" })) {
    items.push({
      kind: "approval",
      approvalId: approval.approvalId,
      operation: approval.operation,
      detail: approval.detail,
      projectId: approval.projectId,
      taskId: approval.taskId,
      createdAt: approval.createdAt,
    })
  }
  for (const status of ["blocked", "failed", "review"] as const) {
    for (const task of listTasksByStatus([status])) {
      items.push({
        kind: status,
        taskId: task.taskId,
        title: task.title,
        status: task.status,
        projectId: task.projectId,
        projectPath: task.projectPath,
        updatedAt: task.updatedAt,
      })
    }
  }
  for (const { message, task } of listQueuedUserMessagesForInbox()) {
    items.push({
      kind: "queued",
      taskId: task.taskId,
      title: task.title,
      messageId: message.id,
      text: truncate(message.text, 500),
      ts: message.ts,
    })
  }
  return items
}

export async function createTaskHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  const rawTitle = typeof params.title === "string" ? params.title.trim() : ""
  if (rawTitle.length === 0 || rawTitle.length > 500) {
    throw invalidParams("title must be a non-empty string (max 500 chars)")
  }
  const goal = optionalText(params.goal, "goal", 4000)
  const rawAuto = params.autoDispatch
  let autoDispatch = true
  if (rawAuto !== undefined && rawAuto !== null) {
    if (typeof rawAuto !== "boolean") throw invalidParams("autoDispatch must be a boolean")
    autoDispatch = rawAuto
  }

  const projectId = resolveProjectId(params, true)
  const task = createTask({ title: rawTitle, projectId, goal: goal ?? null })
  addEvent(
    "task.created",
    `Task ${task.taskId} created: ${task.title}`,
    { taskId: task.taskId, projectId: task.projectId, autoDispatch },
  )

  if (!autoDispatch) {
    return { taskId: task.taskId, status: task.status }
  }

  // Dispatch errors never fail the RPC; they surface as dispatchError.
  try {
    const result = await dispatchTask(task.taskId)
    const payload: Record<string, unknown> = {
      taskId: task.taskId,
      status: result.status,
      ...(result.ceoAgentId ? { ceoAgentId: result.ceoAgentId } : {}),
      ...(result.error ? { dispatchError: result.error } : {}),
    }
    return payload
  } catch (error) {
    const dispatchError = error instanceof Error ? error.message : String(error)
    const current = getTask(task.taskId)
    return { taskId: task.taskId, status: current?.status ?? task.status, dispatchError }
  }
}

export async function listTasksHandler(params: Record<string, unknown>): Promise<{ items: TaskRow[] }> {
  const projectId = resolveProjectId(params, false)
  const status = statusParam(params.status)
  const filter: { projectId?: string; status?: TaskStatus } = {}
  if (projectId !== undefined) filter.projectId = projectId
  if (status !== undefined) filter.status = status
  return { items: listTasks(filter) }
}

export async function getTaskHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  const taskId = requiredTaskId(params.taskId)
  const link = getTaskWithLink(taskId)
  if (!link) throw new RpcError(-32602, `task not found: ${taskId}`)
  return { ...link.task, ceo: link.ceo, pendingMessageCount: link.pendingMessageCount }
}

const UPDATABLE_STATUSES: readonly TaskStatus[] = ["review", "blocked", "done", "failed", "canceled"]

export async function tasksUpdateHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  const taskId = requiredTaskId(params.taskId)
  const rawStatus = params.status
  if (typeof rawStatus !== "string" || !(UPDATABLE_STATUSES as readonly string[]).includes(rawStatus)) {
    throw invalidParams(`status must be one of ${UPDATABLE_STATUSES.join(", ")}`)
  }
  const status = rawStatus as TaskStatus
  const existing = getTask(taskId)
  if (!existing) throw new RpcError(-32602, `task not found: ${taskId}`)
  if ((TERMINAL_TASK_STATUSES as readonly string[]).includes(existing.status)) {
    throw invalidParams(`task ${taskId} is ${existing.status}; done/failed/canceled are terminal`)
  }
  const updated = transitionTaskStatus(taskId, status)!
  const note = optionalText(params.note, "note", 2000)
  if (note) appendBriefDecision(taskId, note)
  return { taskId, status: updated.status }
}

export async function tasksMessageHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  const taskId = requiredTaskId(params.taskId)
  const text = requiredText(params.text, "text", 8000)
  const task = getTask(taskId)
  if (!task) throw new RpcError(-32602, `task not found: ${taskId}`)

  const message = queueTaskMessage({ taskId, direction: "user", text })
  const flush = await deliverQueuedMessages(taskId)
  const deliveredNow = flush.delivered.some((candidate) => candidate.id === message.id)
  if (deliveredNow) {
    return { taskId, outcome: "delivered", messageId: message.id }
  }
  addEvent(
    "task.message_queued",
    `Message ${message.id} queued for task ${taskId}`,
    { taskId, messageId: message.id, direction: "user", reason: flush.error ?? "busy" },
  )
  return { taskId, outcome: "queued", messageId: message.id }
}

export async function tasksMessagesHandler(params: Record<string, unknown>): Promise<{ items: ReturnType<typeof listMessagesForTask> }> {
  const taskId = requiredTaskId(params.taskId)
  const task = getTask(taskId)
  if (!task) throw new RpcError(-32602, `task not found: ${taskId}`)
  return { items: listMessagesForTask(taskId) }
}

function splitLines(value: string | null): string[] {
  if (!value) return []
  return value.split("\n").map((line) => line.trimEnd()).filter((line) => line.length > 0)
}

export async function tasksBriefHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  const rawTaskId = params.taskId
  if (rawTaskId === undefined || rawTaskId === null) {
    // Global overview. Composed from the DB; never round-trips to a CEO.
    const counts = countTasks()
    const needsAction = inboxItems()
    const running = listTasksByStatus(["researching", "planning", "delegating", "running"]).map(briefTaskSummary)
    const done = listTasksByStatus(["done"]).slice(0, 10).map(briefTaskSummary)
    return {
      generatedAt: new Date().toISOString(),
      counts: { ...counts, pendingApprovals: countPendingApprovals() },
      needsAction,
      running,
      done,
    }
  }
  const taskId = requiredTaskId(rawTaskId)
  const link = getTaskWithLink(taskId)
  if (!link) throw new RpcError(-32602, `task not found: ${taskId}`)
  const brief = getTaskBrief(taskId)
  const workers = listTaskAgents(taskId).filter((agent) => agent.role !== "ceo")
  return {
    taskId: link.task.taskId,
    title: link.task.title,
    status: link.task.status,
    goal: link.task.goal,
    summary: brief?.summary ?? null,
    decisions: splitLines(brief?.decisions ?? null),
    blockers: splitLines(brief?.blockers ?? null),
    nextStep: brief?.nextStep ?? null,
    ceo: link.ceo,
    workers,
    pendingMessages: link.pendingMessageCount,
  }
}

export async function tasksInboxHandler(): Promise<{ items: Array<Record<string, unknown>> }> {
  return { items: inboxItems() }
}

export async function cancelTaskHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  const taskId = requiredTaskId(params.taskId)
  const existing = getTask(taskId)
  if (!existing) throw new RpcError(-32602, `task not found: ${taskId}`)
  const task = cancelTask(taskId)
  if (existing.status !== "canceled") {
    addEvent("task.canceled", `Task ${taskId} canceled`, { taskId })
  }
  return { taskId, status: task?.status ?? "canceled" }
}
