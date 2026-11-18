import { RpcError, invalidParams } from "../errors.ts"
import {
  addEvent,
  cancelTask,
  createTask,
  getProject,
  getTask,
  listTasks,
  type TaskRow,
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

export async function createTaskHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  const rawTitle = typeof params.title === "string" ? params.title.trim() : ""
  if (rawTitle.length === 0 || rawTitle.length > 500) {
    throw invalidParams("title must be a non-empty string (max 500 chars)")
  }

  const projectId = resolveProjectId(params, true)
  const task = createTask({ title: rawTitle, projectId })
  addEvent("task.created", `Task ${task.taskId} created: ${task.title}`, { taskId: task.taskId, projectId: task.projectId })

  return { taskId: task.taskId, status: task.status }
}

export async function listTasksHandler(params: Record<string, unknown>): Promise<{ items: TaskRow[] }> {
  const projectId = resolveProjectId(params, false)
  return { items: projectId === undefined ? listTasks({}) : listTasks({ projectId }) }
}

export async function getTaskHandler(params: Record<string, unknown>): Promise<TaskRow> {
  const taskId = typeof params.taskId === "string" ? params.taskId.trim() : ""
  if (!taskId) throw invalidParams("taskId is required")
  const task = getTask(taskId)
  if (!task) throw new RpcError(-32602, `task not found: ${taskId}`)
  return task
}

export async function cancelTaskHandler(params: Record<string, unknown>): Promise<Record<string, unknown>> {
  const taskId = typeof params.taskId === "string" ? params.taskId.trim() : ""
  if (!taskId) throw invalidParams("taskId is required")
  const existing = getTask(taskId)
  if (!existing) throw new RpcError(-32602, `task not found: ${taskId}`)
  const task = cancelTask(taskId)
  if (existing.status !== "canceled") {
    addEvent("task.canceled", `Task ${taskId} canceled`, { taskId })
  }
  return { taskId, status: task?.status ?? "canceled" }
}
