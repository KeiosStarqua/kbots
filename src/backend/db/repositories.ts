import type { Database } from "bun:sqlite"
import { openDatabase } from "./database.ts"

export interface ProjectRow {
  projectId: string
  name: string
  path: string
  botRoles: string[]
  createdAt: string
  updatedAt: string
}

export type TaskStatus =
  | "queued"
  | "researching"
  | "planning"
  | "delegating"
  | "running"
  | "review"
  | "blocked"
  | "done"
  | "failed"
  | "canceled"

export const TASK_STATUSES: readonly TaskStatus[] = [
  "queued",
  "researching",
  "planning",
  "delegating",
  "running",
  "review",
  "blocked",
  "done",
  "failed",
  "canceled",
]

export const TERMINAL_TASK_STATUSES: readonly TaskStatus[] = ["done", "failed", "canceled"]

export interface TaskRow {
  taskId: string
  title: string
  status: TaskStatus
  projectId: string | null
  projectPath: string | null
  createdAt: string
  updatedAt: string
  canceledAt: string | null
  goal: string | null
  detail: string | null
}

export type ApprovalStatus = "pending" | "approved"

export interface ApprovalRow {
  approvalId: string
  operation: string
  detail: string | null
  projectId: string | null
  taskId: string | null
  status: ApprovalStatus
  createdAt: string
  decidedAt: string | null
}

export interface EventRow {
  id: number
  ts: string
  kind: string
  message: string
  data: unknown
}

let singleton: Database | null = null

export function getDb(): Database {
  if (!singleton) singleton = openDatabase()
  return singleton
}

function now(): string {
  return new Date().toISOString()
}

function parseJson(value: string | null): unknown {
  if (value === null) return null
  try {
    return JSON.parse(value)
  } catch {
    return null
  }
}

function parseStringArray(value: string | null): string[] {
  const parsed = parseJson(value)
  if (!Array.isArray(parsed)) return []
  return parsed.filter((item): item is string => typeof item === "string")
}

interface ProjectRecord {
  id: string
  name: string
  path: string
  bot_roles: string
  created_at: string
  updated_at: string
}

interface TaskRecord {
  id: string
  title: string
  status: string
  project_id: string | null
  project_path: string | null
  created_at: string
  updated_at: string
  canceled_at: string | null
  goal: string | null
  detail: string | null
}

interface ApprovalRecord {
  id: string
  operation: string
  detail: string | null
  project_id: string | null
  task_id: string | null
  status: string
  created_at: string
  decided_at: string | null
}

interface EventRecord {
  id: number
  ts: string
  kind: string
  message: string
  data: string | null
}

function toProjectRow(record: ProjectRecord): ProjectRow {
  return {
    projectId: record.id,
    name: record.name,
    path: record.path,
    botRoles: parseStringArray(record.bot_roles),
    createdAt: record.created_at,
    updatedAt: record.updated_at,
  }
}

function toTaskStatus(value: string): TaskStatus {
  return (TASK_STATUSES as readonly string[]).includes(value) ? (value as TaskStatus) : "queued"
}

function toTaskRow(record: TaskRecord): TaskRow {
  return {
    taskId: record.id,
    title: record.title,
    status: toTaskStatus(record.status),
    projectId: record.project_id,
    projectPath: record.project_path,
    createdAt: record.created_at,
    updatedAt: record.updated_at,
    canceledAt: record.canceled_at,
    goal: record.goal ?? null,
    detail: record.detail ?? null,
  }
}

function toApprovalStatus(value: string): ApprovalStatus {
  return value === "approved" ? "approved" : "pending"
}

function toApprovalRow(record: ApprovalRecord): ApprovalRow {
  return {
    approvalId: record.id,
    operation: record.operation,
    detail: record.detail,
    projectId: record.project_id,
    taskId: record.task_id,
    status: toApprovalStatus(record.status),
    createdAt: record.created_at,
    decidedAt: record.decided_at,
  }
}

function toEventRow(record: EventRecord): EventRow {
  return {
    id: record.id,
    ts: record.ts,
    kind: record.kind,
    message: record.message,
    data: parseJson(record.data),
  }
}

export function nextId(prefix: string, db: Database = getDb()): string {
  const run = db.transaction((key: string): string => {
    const row = db.query("SELECT value FROM meta WHERE key = ?").get(key) as { value: string } | null
    const current = row ? Number.parseInt(row.value, 10) : 0
    const next = Number.isFinite(current) ? current + 1 : 1
    db.query(
      "INSERT INTO meta(key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    ).run(key, String(next))
    return `${prefix}-${next}`
  })
  return run(`sequence:${prefix}`)
}

export function upsertProject(
  input: { path: string; name: string; botRoles: string[] },
  db: Database = getDb(),
): { project: ProjectRow; created: boolean } {
  const run = db.transaction((): { project: ProjectRow; created: boolean } => {
    const timestamp = now()
    const existing = db
      .query("SELECT * FROM projects WHERE path = ?")
      .get(input.path) as ProjectRecord | null
    if (existing) {
      db.query("UPDATE projects SET name = ?, bot_roles = ?, updated_at = ? WHERE path = ?").run(
        input.name,
        JSON.stringify(input.botRoles),
        timestamp,
        input.path,
      )
      const updated = db
        .query("SELECT * FROM projects WHERE path = ?")
        .get(input.path) as ProjectRecord
      return { project: toProjectRow(updated), created: false }
    }
    const id = nextId("PRJ", db)
    db.query(
      "INSERT INTO projects(id, name, path, bot_roles, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
    ).run(id, input.name, input.path, JSON.stringify(input.botRoles), timestamp, timestamp)
    const inserted = db.query("SELECT * FROM projects WHERE id = ?").get(id) as ProjectRecord
    return { project: toProjectRow(inserted), created: true }
  })
  return run()
}

export function listProjects(db: Database = getDb()): ProjectRow[] {
  const rows = db
    .query("SELECT * FROM projects ORDER BY created_at ASC, id ASC")
    .all() as ProjectRecord[]
  return rows.map(toProjectRow)
}

export function getProject(ref: string, db: Database = getDb()): ProjectRow | null {
  const byId = db.query("SELECT * FROM projects WHERE id = ?").get(ref) as ProjectRecord | null
  if (byId) return toProjectRow(byId)
  const byName = db.query("SELECT * FROM projects WHERE name = ?").get(ref) as ProjectRecord | null
  if (byName) return toProjectRow(byName)
  const byPath = db.query("SELECT * FROM projects WHERE path = ?").get(ref) as ProjectRecord | null
  if (byPath) return toProjectRow(byPath)
  return null
}

export function getProjectByPath(path: string, db: Database = getDb()): ProjectRow | null {
  const row = db.query("SELECT * FROM projects WHERE path = ?").get(path) as ProjectRecord | null
  return row ? toProjectRow(row) : null
}

export function countProjects(db: Database = getDb()): number {
  const row = db.query("SELECT COUNT(*) AS count FROM projects").get() as { count: number }
  return row.count
}

const TASK_SELECT = `
  SELECT tasks.*, projects.path AS project_path
  FROM tasks
  LEFT JOIN projects ON projects.id = tasks.project_id
`

export function createTask(
  input: { title: string; projectId?: string | null; goal?: string | null; detail?: string | null },
  db: Database = getDb(),
): TaskRow {
  const run = db.transaction((): TaskRow => {
    const timestamp = now()
    const id = nextId("TASK", db)
    db.query(
      "INSERT INTO tasks(id, title, status, project_id, created_at, updated_at, canceled_at, goal, detail) VALUES (?, ?, 'queued', ?, ?, ?, NULL, ?, ?)",
    ).run(id, input.title, input.projectId ?? null, timestamp, timestamp, input.goal ?? null, input.detail ?? null)
    const row = db.query(`${TASK_SELECT} WHERE tasks.id = ?`).get(id) as TaskRecord
    return toTaskRow(row)
  })
  return run()
}

export function getTask(taskId: string, db: Database = getDb()): TaskRow | null {
  const row = db.query(`${TASK_SELECT} WHERE tasks.id = ?`).get(taskId) as TaskRecord | null
  return row ? toTaskRow(row) : null
}

export function listTasks(
  filter?: { projectId?: string; status?: TaskStatus },
  db: Database = getDb(),
): TaskRow[] {
  const clauses: string[] = []
  const values: string[] = []
  if (filter?.projectId !== undefined) {
    clauses.push("tasks.project_id = ?")
    values.push(filter.projectId)
  }
  if (filter?.status !== undefined) {
    clauses.push("tasks.status = ?")
    values.push(filter.status)
  }
  const where = clauses.length > 0 ? ` WHERE ${clauses.join(" AND ")}` : ""
  const rows = db
    .query(`${TASK_SELECT}${where} ORDER BY tasks.created_at DESC, tasks.id DESC`)
    .all(...values) as TaskRecord[]
  return rows.map(toTaskRow)
}

export function listTasksByStatus(
  statuses: readonly TaskStatus[],
  db: Database = getDb(),
): TaskRow[] {
  if (statuses.length === 0) return []
  const placeholders = statuses.map(() => "?").join(", ")
  const rows = db
    .query(`${TASK_SELECT} WHERE tasks.status IN (${placeholders}) ORDER BY tasks.created_at DESC, tasks.id DESC`)
    .all(...statuses) as TaskRecord[]
  return rows.map(toTaskRow)
}

// ---------------------------------------------------------------------------
// tasks v4: links, messages, briefs, status transitions
// ---------------------------------------------------------------------------

export interface TaskAgentLinkRow {
  taskId: string
  agentId: string
  role: "ceo" | "worker" | "git" | "ops"
  createdAt: string
}

interface TaskAgentLinkRecord {
  task_id: string
  agent_id: string
  role: string
  created_at: string
}

function toTaskAgentLinkRow(record: TaskAgentLinkRecord): TaskAgentLinkRow {
  const role = record.role === "ceo" || record.role === "worker" || record.role === "git" || record.role === "ops"
    ? record.role
    : "worker"
  return { taskId: record.task_id, agentId: record.agent_id, role, createdAt: record.created_at }
}

export function insertTaskAgent(
  input: { taskId: string; agentId: string; role: "ceo" | "worker" | "git" | "ops" },
  db: Database = getDb(),
): TaskAgentLinkRow {
  const timestamp = now()
  db.query(
    "INSERT INTO task_agents(task_id, agent_id, role, created_at) VALUES (?, ?, ?, ?)",
  ).run(input.taskId, input.agentId, input.role, timestamp)
  const row = db
    .query("SELECT * FROM task_agents WHERE task_id = ? AND agent_id = ?")
    .get(input.taskId, input.agentId) as TaskAgentLinkRecord
  return toTaskAgentLinkRow(row)
}

export function getTaskAgentLink(
  taskId: string,
  role?: "ceo" | "worker" | "git" | "ops",
  db: Database = getDb(),
): TaskAgentLinkRow | null {
  const row = role === undefined
    ? db.query("SELECT * FROM task_agents WHERE task_id = ? ORDER BY created_at ASC LIMIT 1").get(taskId) as TaskAgentLinkRecord | null
    : db.query("SELECT * FROM task_agents WHERE task_id = ? AND role = ?").get(taskId, role) as TaskAgentLinkRecord | null
  return row ? toTaskAgentLinkRow(row) : null
}

/** Reverse lookup for a linked task (used to tag agent events with taskId). */
export function getTaskIdForAgent(
  agentId: string,
  role?: "ceo" | "worker" | "git" | "ops",
  db: Database = getDb(),
): string | null {
  const row = role === undefined
    ? db.query("SELECT task_id FROM task_agents WHERE agent_id = ? ORDER BY created_at ASC LIMIT 1").get(agentId) as { task_id: string } | null
    : db.query("SELECT task_id FROM task_agents WHERE agent_id = ? AND role = ?").get(agentId, role) as { task_id: string } | null
  return row?.task_id ?? null
}

export function listTaskAgents(taskId: string, db: Database = getDb()): AgentRow[] {
  const rows = db
    .query(
      `SELECT agents.* FROM agents
       JOIN task_agents ON task_agents.agent_id = agents.id
       WHERE task_agents.task_id = ?
       ORDER BY task_agents.created_at ASC, agents.id ASC`,
    )
    .all(taskId) as AgentRecord[]
  return rows.map(toAgentRow)
}

export function getTaskCeoAgent(taskId: string, db: Database = getDb()): AgentRow | null {
  const row = db
    .query(
      `SELECT agents.* FROM agents
       JOIN task_agents ON task_agents.agent_id = agents.id
       WHERE task_agents.task_id = ? AND task_agents.role = 'ceo'
       ORDER BY task_agents.created_at DESC LIMIT 1`,
    )
    .get(taskId) as AgentRecord | null
  return row ? toAgentRow(row) : null
}

// CEOs whose last probed session outcome is a finished/unreachable state do
// not consume an inference slot; only NULL (never probed) or in-flight
// outcomes count. "unreachable" is excluded: a dead session holds no slot.
export function countActiveCeoAgents(projectId: string, db: Database = getDb()): number {
  const row = db
    .query(
      `SELECT COUNT(*) AS count FROM agents
       WHERE project_id = ? AND role = 'ceo' AND status IN ('starting','running','stopping')
         AND (last_outcome IS NULL
              OR last_outcome NOT IN ('succeeded','failed','canceled','ended','aborted','error','unreachable'))`,
    )
    .get(projectId) as { count: number }
  return row.count
}

export type TaskMessageDirection = "user" | "system"
export type TaskMessageStatus = "queued" | "delivered" | "canceled"

export interface TaskMessageRow {
  id: string
  taskId: string
  ts: string
  direction: TaskMessageDirection
  text: string
  status: TaskMessageStatus
  deliveredAt: string | null
  attempts: number
}

interface TaskMessageRecord {
  id: string
  task_id: string
  ts: string
  direction: string
  text: string
  status: string
  delivered_at: string | null
  attempts: number
}

function toTaskMessageRow(record: TaskMessageRecord): TaskMessageRow {
  return {
    id: record.id,
    taskId: record.task_id,
    ts: record.ts,
    direction: record.direction === "system" ? "system" : "user",
    text: record.text,
    status: record.status === "delivered" || record.status === "canceled" ? record.status : "queued",
    deliveredAt: record.delivered_at,
    attempts: typeof record.attempts === "number" && Number.isFinite(record.attempts) ? record.attempts : 0,
  }
}

export function queueTaskMessage(
  input: { taskId: string; direction: TaskMessageDirection; text: string },
  db: Database = getDb(),
): TaskMessageRow {
  const id = nextId("MSG", db)
  const timestamp = now()
  db.query(
    "INSERT INTO task_messages(id, task_id, ts, direction, text, status, delivered_at) VALUES (?, ?, ?, ?, ?, 'queued', NULL)",
  ).run(id, input.taskId, timestamp, input.direction, input.text)
  const row = db.query("SELECT * FROM task_messages WHERE id = ?").get(id) as TaskMessageRecord
  return toTaskMessageRow(row)
}

export function listQueuedMessagesForTask(taskId: string, db: Database = getDb()): TaskMessageRow[] {
  const rows = db
    .query("SELECT * FROM task_messages WHERE task_id = ? AND status = 'queued' ORDER BY ts ASC, id ASC")
    .all(taskId) as TaskMessageRecord[]
  return rows.map(toTaskMessageRow)
}

export function listMessagesForTask(
  taskId: string,
  limit = 100,
  db: Database = getDb(),
): TaskMessageRow[] {
  const rows = db
    .query("SELECT * FROM task_messages WHERE task_id = ? ORDER BY ts ASC, id ASC LIMIT ?")
    .all(taskId, limit) as TaskMessageRecord[]
  return rows.map(toTaskMessageRow)
}

/** Bump the delivery-attempt counter; returns the updated row. */
export function incrementMessageAttempts(
  messageId: string,
  db: Database = getDb(),
): TaskMessageRow | null {
  db.query("UPDATE task_messages SET attempts = attempts + 1 WHERE id = ?").run(messageId)
  const row = db.query("SELECT * FROM task_messages WHERE id = ?").get(messageId) as TaskMessageRecord | null
  return row ? toTaskMessageRow(row) : null
}

/** Mark a queued message canceled (existing status vocabulary). */
export function cancelTaskMessage(
  messageId: string,
  db: Database = getDb(),
): TaskMessageRow | null {
  db.query("UPDATE task_messages SET status = 'canceled' WHERE id = ? AND status = 'queued'").run(messageId)
  const row = db.query("SELECT * FROM task_messages WHERE id = ?").get(messageId) as TaskMessageRecord | null
  return row ? toTaskMessageRow(row) : null
}

export const MESSAGE_ATTEMPT_LIMIT = 5
const DELIVERY_BLOCKABLE_TASK_STATUSES: readonly TaskStatus[] = [
  "researching",
  "planning",
  "delegating",
  "running",
]

export const DELIVERY_FAILURE_BLOCKER =
  "system: delivery failed after 5 attempts — OpenCode unresponsive or refusing"

/**
 * Give up on a queued message: cancel it, log `message.abandoned`, and move
 * the task to `blocked` (with a note in task_briefs.blockers) when it is
 * actively researching/planning/delegating/running. Best effort semantics —
 * callers hold the taskId for context.
 */
export function abandonQueuedMessage(
  message: TaskMessageRow,
  db: Database = getDb(),
): void {
  cancelTaskMessage(message.id, db)
  addEvent(
    "message.abandoned",
    `Message ${message.id} abandoned after ${message.attempts} attempts`,
    { taskId: message.taskId, messageId: message.id, attempts: message.attempts },
    db,
  )
  const task = getTask(message.taskId, db)
  if (task && (DELIVERY_BLOCKABLE_TASK_STATUSES as readonly string[]).includes(task.status)) {
    appendBriefBlocker(message.taskId, DELIVERY_FAILURE_BLOCKER, db)
    transitionTaskStatus(message.taskId, "blocked", db)
  }
}

export function markMessageDelivered(
  messageId: string,
  db: Database = getDb(),
): TaskMessageRow | null {
  const timestamp = now()
  db.query(
    "UPDATE task_messages SET status = 'delivered', delivered_at = ? WHERE id = ? AND status = 'queued'",
  ).run(timestamp, messageId)
  const row = db.query("SELECT * FROM task_messages WHERE id = ?").get(messageId) as TaskMessageRecord | null
  return row ? toTaskMessageRow(row) : null
}

export function countQueuedMessagesForTask(taskId: string, db: Database = getDb()): number {
  const row = db
    .query("SELECT COUNT(*) AS count FROM task_messages WHERE task_id = ? AND status = 'queued'")
    .get(taskId) as { count: number }
  return row.count
}

export function listTasksWithQueuedMessages(db: Database = getDb()): TaskRow[] {
  const rows = db
    .query(
      `${TASK_SELECT}
       WHERE tasks.id IN (SELECT DISTINCT task_id FROM task_messages WHERE status = 'queued')
       ORDER BY tasks.created_at ASC, tasks.id ASC`,
    )
    .all() as TaskRecord[]
  return rows.map(toTaskRow)
}

export function listQueuedUserMessagesForInbox(
  db: Database = getDb(),
): Array<{ message: TaskMessageRow; task: TaskRow }> {
  const rows = db
    .query(
      `SELECT m.*, t.id AS t_id, t.title AS t_title, t.status AS t_status, t.project_id AS t_project_id,
              t.created_at AS t_created_at, t.updated_at AS t_updated_at, t.canceled_at AS t_canceled_at,
              t.goal AS t_goal, t.detail AS t_detail, projects.path AS t_project_path
       FROM task_messages m
       JOIN tasks t ON t.id = m.task_id
       LEFT JOIN projects ON projects.id = t.project_id
       WHERE m.status = 'queued' AND m.direction = 'user'
       ORDER BY m.ts ASC, m.id ASC`,
    )
    .all() as Array<TaskMessageRecord & {
      t_id: string
      t_title: string
      t_status: string
      t_project_id: string | null
      t_created_at: string
      t_updated_at: string
      t_canceled_at: string | null
      t_goal: string | null
      t_detail: string | null
      t_project_path: string | null
    }>
  return rows.map((row) => ({
    message: toTaskMessageRow(row),
    task: toTaskRow({
      id: row.t_id,
      title: row.t_title,
      status: row.t_status,
      project_id: row.t_project_id,
      project_path: row.t_project_path,
      created_at: row.t_created_at,
      updated_at: row.t_updated_at,
      canceled_at: row.t_canceled_at,
      goal: row.t_goal,
      detail: row.t_detail,
    }),
  }))
}

export interface TaskWithLink {
  task: TaskRow
  ceo: AgentRow | null
  pendingMessageCount: number
}

export function getTaskWithLink(taskId: string, db: Database = getDb()): TaskWithLink | null {
  const task = getTask(taskId, db)
  if (!task) return null
  return {
    task,
    ceo: getTaskCeoAgent(taskId, db),
    pendingMessageCount: countQueuedMessagesForTask(taskId, db),
  }
}

export interface TaskBriefRow {
  taskId: string
  summary: string | null
  decisions: string | null
  blockers: string | null
  nextStep: string | null
  updatedAt: string
}

interface TaskBriefRecord {
  task_id: string
  summary: string | null
  decisions: string | null
  blockers: string | null
  next_step: string | null
  updated_at: string
}

function toTaskBriefRow(record: TaskBriefRecord): TaskBriefRow {
  return {
    taskId: record.task_id,
    summary: record.summary,
    decisions: record.decisions,
    blockers: record.blockers,
    nextStep: record.next_step,
    updatedAt: record.updated_at,
  }
}

export function getTaskBrief(taskId: string, db: Database = getDb()): TaskBriefRow | null {
  const row = db.query("SELECT * FROM task_briefs WHERE task_id = ?").get(taskId) as TaskBriefRecord | null
  return row ? toTaskBriefRow(row) : null
}

/** Selective merge: only the provided fields are written; updated_at always bumps. */
export function upsertTaskBrief(
  patch: {
    taskId: string
    summary?: string | null
    decisions?: string | null
    blockers?: string | null
    nextStep?: string | null
  },
  db: Database = getDb(),
): TaskBriefRow {
  const timestamp = now()
  const existing = getTaskBrief(patch.taskId, db)
  const summary = patch.summary !== undefined ? patch.summary : (existing?.summary ?? null)
  const decisions = patch.decisions !== undefined ? patch.decisions : (existing?.decisions ?? null)
  const blockers = patch.blockers !== undefined ? patch.blockers : (existing?.blockers ?? null)
  const nextStep = patch.nextStep !== undefined ? patch.nextStep : (existing?.nextStep ?? null)
  db.query(
    `INSERT INTO task_briefs(task_id, summary, decisions, blockers, next_step, updated_at)
     VALUES (?, ?, ?, ?, ?, ?)
     ON CONFLICT(task_id) DO UPDATE SET
       summary = excluded.summary,
       decisions = excluded.decisions,
       blockers = excluded.blockers,
       next_step = excluded.next_step,
       updated_at = excluded.updated_at`,
  ).run(patch.taskId, summary, decisions, blockers, nextStep, timestamp)
  addEvent("brief.updated", `Brief for task ${patch.taskId} updated`, { taskId: patch.taskId }, db)
  return getTaskBrief(patch.taskId, db)!
}

/** Append one line to the brief's decisions (newest last). */
export function appendBriefDecision(
  taskId: string,
  note: string,
  db: Database = getDb(),
): TaskBriefRow {
  const line = note.trim()
  const existing = getTaskBrief(taskId, db)
  if (line.length === 0) {
    return existing ?? upsertTaskBrief({ taskId }, db)
  }
  const decisions = existing?.decisions
    ? `${existing.decisions.replace(/\n+$/, "")}\n${line}`
    : line
  return upsertTaskBrief({ taskId, decisions }, db)
}

/** Append one line to the brief's blockers (newest last). */
export function appendBriefBlocker(
  taskId: string,
  note: string,
  db: Database = getDb(),
): TaskBriefRow {
  const line = note.trim()
  const existing = getTaskBrief(taskId, db)
  if (line.length === 0) {
    return existing ?? upsertTaskBrief({ taskId }, db)
  }
  const blockers = existing?.blockers
    ? `${existing.blockers.replace(/\n+$/, "")}\n${line}`
    : line
  return upsertTaskBrief({ taskId, blockers }, db)
}

/**
 * Move a task to a new status. Logs a `task.status` event when the status
 * actually changes. `canceled` stamps canceled_at.
 */
export function transitionTaskStatus(
  taskId: string,
  status: TaskStatus,
  db: Database = getDb(),
): TaskRow | null {
  const existing = getTask(taskId, db)
  if (!existing) return null
  if (existing.status === status) return existing
  const timestamp = now()
  const canceledAt = status === "canceled" ? timestamp : existing.canceledAt
  db.query(
    "UPDATE tasks SET status = ?, canceled_at = ?, updated_at = ? WHERE id = ?",
  ).run(status, canceledAt, timestamp, taskId)
  const updated = getTask(taskId, db)!
  addEvent(
    "task.status",
    `Task ${taskId} ${existing.status} → ${status}`,
    { taskId, from: existing.status, to: status },
    db,
  )
  return updated
}

export function cancelTask(taskId: string, db: Database = getDb()): TaskRow | null {
  const existing = getTask(taskId, db)
  if (!existing) return null
  if (existing.status !== "canceled") {
    const timestamp = now()
    db.query(
      "UPDATE tasks SET status = 'canceled', canceled_at = ?, updated_at = ? WHERE id = ? AND status != 'canceled'",
    ).run(timestamp, timestamp, taskId)
    return getTask(taskId, db)
  }
  return existing
}

export function countTasks(
  db: Database = getDb(),
): { total: number; byStatus: Record<string, number> } {
  const totalRow = db.query("SELECT COUNT(*) AS count FROM tasks").get() as { count: number }
  const rows = db
    .query("SELECT status, COUNT(*) AS count FROM tasks GROUP BY status")
    .all() as Array<{ status: string; count: number }>
  const byStatus: Record<string, number> = {}
  for (const row of rows) byStatus[row.status] = row.count
  return { total: totalRow.count, byStatus }
}

export function listApprovals(
  filter?: { status?: string },
  db: Database = getDb(),
): ApprovalRow[] {
  const order = "ORDER BY CASE WHEN status = 'pending' THEN 0 ELSE 1 END, created_at DESC, id DESC"
  if (filter?.status !== undefined) {
    const rows = db
      .query(`SELECT * FROM approvals WHERE status = ? ${order}`)
      .all(filter.status) as ApprovalRecord[]
    return rows.map(toApprovalRow)
  }
  const rows = db.query(`SELECT * FROM approvals ${order}`).all() as ApprovalRecord[]
  return rows.map(toApprovalRow)
}

export function getApproval(
  approvalId: string,
  db: Database = getDb(),
): ApprovalRow | null {
  const row = db
    .query("SELECT * FROM approvals WHERE id = ?")
    .get(approvalId) as ApprovalRecord | null
  return row ? toApprovalRow(row) : null
}

export function approveApproval(
  approvalId: string,
  db: Database = getDb(),
): ApprovalRow | null {
  const existing = getApproval(approvalId, db)
  if (!existing) return null
  if (existing.status === "approved") return existing
  const timestamp = now()
  db.query(
    "UPDATE approvals SET status = 'approved', decided_at = ? WHERE id = ? AND status = 'pending'",
  ).run(timestamp, approvalId)
  return getApproval(approvalId, db)
}

export function countPendingApprovals(db: Database = getDb()): number {
  const row = db
    .query("SELECT COUNT(*) AS count FROM approvals WHERE status = 'pending'")
    .get() as { count: number }
  return row.count
}

export function addEvent(
  kind: string,
  message: string,
  data?: unknown,
  db: Database = getDb(),
): EventRow {
  const timestamp = now()
  const serialized = data === undefined ? null : JSON.stringify(data)
  const result = db
    .query("INSERT INTO events(ts, kind, message, data) VALUES (?, ?, ?, ?)")
    .run(timestamp, kind, message, serialized)
  const id = Number(result.lastInsertRowid)
  const row = db.query("SELECT * FROM events WHERE id = ?").get(id) as EventRecord
  return toEventRow(row)
}

export function listEvents(limit: number, db: Database = getDb()): EventRow[] {
  const rows = db
    .query("SELECT * FROM events ORDER BY id DESC LIMIT ?")
    .all(limit) as EventRecord[]
  return rows.map(toEventRow)
}

export type AgentStatus = "starting" | "running" | "stopping" | "stopped" | "error"
export type AgentMode = "headless" | "terminal"
export type AgentRole = "ceo" | "git" | "ops" | "worker"

export interface AgentRow {
  id: string
  projectId: string
  role: AgentRole
  mode: AgentMode
  directory: string
  status: AgentStatus
  sessionId: string | null
  hcAddress: string | null
  name: string | null
  createdAt: string
  updatedAt: string
  stoppedAt: string | null
  lastError: string | null
  lastOutcome: string | null
  lastOutcomeAt: string | null
}

interface AgentRecord {
  id: string
  project_id: string
  role: string
  mode: string
  directory: string
  status: string
  session_id: string | null
  hc_address: string | null
  name: string | null
  created_at: string
  updated_at: string
  stopped_at: string | null
  last_error: string | null
  last_outcome: string | null
  last_outcome_at: string | null
}

const AGENT_ROLES: readonly AgentRole[] = ["ceo", "git", "ops", "worker"]
const AGENT_MODES: readonly AgentMode[] = ["headless", "terminal"]
const AGENT_STATUSES: readonly AgentStatus[] = ["starting", "running", "stopping", "stopped", "error"]
const LIVE_AGENT_STATUSES: readonly AgentStatus[] = ["starting", "running", "stopping"]

function toAgentRow(record: AgentRecord): AgentRow {
  return {
    id: record.id,
    projectId: record.project_id,
    role: (AGENT_ROLES as readonly string[]).includes(record.role) ? (record.role as AgentRole) : "worker",
    mode: (AGENT_MODES as readonly string[]).includes(record.mode) ? (record.mode as AgentMode) : "headless",
    directory: record.directory,
    status: (AGENT_STATUSES as readonly string[]).includes(record.status) ? (record.status as AgentStatus) : "error",
    sessionId: record.session_id,
    hcAddress: record.hc_address,
    name: record.name,
    createdAt: record.created_at,
    updatedAt: record.updated_at,
    stoppedAt: record.stopped_at,
    lastError: record.last_error,
    lastOutcome: record.last_outcome ?? null,
    lastOutcomeAt: record.last_outcome_at ?? null,
  }
}

export function createAgent(
  input: {
    projectId: string
    role: AgentRole
    mode: AgentMode
    directory: string
    name?: string | null
    sessionId?: string | null
    hcAddress?: string | null
    status?: AgentStatus
  },
  db: Database = getDb(),
): AgentRow {
  const run = db.transaction((): AgentRow => {
    const timestamp = now()
    const id = nextId("AGT", db)
    db.query(
      `INSERT INTO agents(id, project_id, role, mode, directory, status, session_id, hc_address, name, created_at, updated_at, stopped_at, last_error)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL)`,
    ).run(
      id,
      input.projectId,
      input.role,
      input.mode,
      input.directory,
      input.status ?? "starting",
      input.sessionId ?? null,
      input.hcAddress ?? null,
      input.name ?? null,
      timestamp,
      timestamp,
    )
    const row = db.query("SELECT * FROM agents WHERE id = ?").get(id) as AgentRecord
    return toAgentRow(row)
  })
  return run()
}

export function getAgent(agentId: string, db: Database = getDb()): AgentRow | null {
  const row = db.query("SELECT * FROM agents WHERE id = ?").get(agentId) as AgentRecord | null
  return row ? toAgentRow(row) : null
}

export function getActiveAgentByProjectRole(
  projectId: string,
  role: string,
  db: Database = getDb(),
): AgentRow | null {
  const row = db
    .query("SELECT * FROM agents WHERE project_id = ? AND role = ? AND status IN ('starting','running','stopping')")
    .get(projectId, role) as AgentRecord | null
  return row ? toAgentRow(row) : null
}

export function getActiveAgentByName(name: string, db: Database = getDb()): AgentRow | null {
  const row = db
    .query("SELECT * FROM agents WHERE name = ? AND status IN ('starting','running','stopping')")
    .get(name) as AgentRecord | null
  return row ? toAgentRow(row) : null
}

export function getLatestAgentByProjectRole(
  projectId: string,
  role: string,
  db: Database = getDb(),
): AgentRow | null {
  const row = db
    .query("SELECT * FROM agents WHERE project_id = ? AND role = ? ORDER BY created_at DESC, id DESC LIMIT 1")
    .get(projectId, role) as AgentRecord | null
  return row ? toAgentRow(row) : null
}

export function updateAgent(
  agentId: string,
  patch: Partial<{
    status: AgentStatus
    mode: AgentMode
    sessionId: string | null
    hcAddress: string | null
    name: string | null
    stoppedAt: string | null
    lastError: string | null
    lastOutcome: string | null
    lastOutcomeAt: string | null
  }>,
  db: Database = getDb(),
): AgentRow | null {
  const sets: string[] = []
  const values: (string | null)[] = []
  if ("status" in patch) { sets.push("status = ?"); values.push(patch.status ?? null) }
  if ("mode" in patch) { sets.push("mode = ?"); values.push(patch.mode ?? null) }
  if ("sessionId" in patch) { sets.push("session_id = ?"); values.push(patch.sessionId ?? null) }
  if ("hcAddress" in patch) { sets.push("hc_address = ?"); values.push(patch.hcAddress ?? null) }
  if ("name" in patch) { sets.push("name = ?"); values.push(patch.name ?? null) }
  if ("stoppedAt" in patch) { sets.push("stopped_at = ?"); values.push(patch.stoppedAt ?? null) }
  if ("lastError" in patch) { sets.push("last_error = ?"); values.push(patch.lastError ?? null) }
  if ("lastOutcome" in patch) { sets.push("last_outcome = ?"); values.push(patch.lastOutcome ?? null) }
  if ("lastOutcomeAt" in patch) { sets.push("last_outcome_at = ?"); values.push(patch.lastOutcomeAt ?? null) }
  if (sets.length === 0) return getAgent(agentId, db)
  sets.push("updated_at = ?")
  values.push(now())
  values.push(agentId)
  db.query(`UPDATE agents SET ${sets.join(", ")} WHERE id = ?`).run(...values)
  return getAgent(agentId, db)
}

export function listAgents(db: Database = getDb()): AgentRow[] {
  const rows = db
    .query("SELECT * FROM agents ORDER BY created_at ASC, id ASC")
    .all() as AgentRecord[]
  return rows.map(toAgentRow)
}

export function listStartingAgents(db: Database = getDb()): AgentRow[] {
  const rows = db
    .query("SELECT * FROM agents WHERE status = 'starting'")
    .all() as AgentRecord[]
  return rows.map(toAgentRow)
}

export function listLiveAgents(db: Database = getDb()): AgentRow[] {
  const rows = db
    .query("SELECT * FROM agents WHERE status IN ('starting','running','stopping')")
    .all() as AgentRecord[]
  return rows.map(toAgentRow)
}

export function isLiveAgentStatus(status: AgentStatus): boolean {
  return (LIVE_AGENT_STATUSES as readonly string[]).includes(status)
}

export interface WatcherRow {
  projectId: string
  enabled: boolean
  intervalSeconds: number
  debounceSeconds: number
  cooldownSeconds: number
  maxPaths: number
  observedFingerprint: string | null
  reportedFingerprint: string | null
  lastChangeAt: string | null
  lastDeliveredAt: string | null
  createdAt: string
  updatedAt: string
}

interface WatcherRecord {
  project_id: string
  enabled: number
  interval_seconds: number
  debounce_seconds: number
  cooldown_seconds: number
  max_paths: number
  observed_fingerprint: string | null
  reported_fingerprint: string | null
  last_change_at: string | null
  last_delivered_at: string | null
  created_at: string
  updated_at: string
}

function toWatcherRow(record: WatcherRecord): WatcherRow {
  return {
    projectId: record.project_id,
    enabled: record.enabled !== 0,
    intervalSeconds: record.interval_seconds,
    debounceSeconds: record.debounce_seconds,
    cooldownSeconds: record.cooldown_seconds,
    maxPaths: record.max_paths,
    observedFingerprint: record.observed_fingerprint,
    reportedFingerprint: record.reported_fingerprint,
    lastChangeAt: record.last_change_at,
    lastDeliveredAt: record.last_delivered_at,
    createdAt: record.created_at,
    updatedAt: record.updated_at,
  }
}

export function upsertWatcher(
  input: {
    projectId: string
    enabled: boolean
    intervalSeconds?: number
    debounceSeconds?: number
    cooldownSeconds?: number
    maxPaths?: number
  },
  db: Database = getDb(),
): WatcherRow {
  const run = db.transaction((): WatcherRow => {
    const timestamp = now()
    const existing = getWatcher(input.projectId, db)
    if (existing) {
      db.query(
        `UPDATE watchers SET enabled = ?,
           interval_seconds = COALESCE(?, interval_seconds),
           debounce_seconds = COALESCE(?, debounce_seconds),
           cooldown_seconds = COALESCE(?, cooldown_seconds),
           max_paths = COALESCE(?, max_paths),
           updated_at = ?
         WHERE project_id = ?`,
      ).run(
        input.enabled ? 1 : 0,
        input.intervalSeconds ?? null,
        input.debounceSeconds ?? null,
        input.cooldownSeconds ?? null,
        input.maxPaths ?? null,
        timestamp,
        input.projectId,
      )
      return getWatcher(input.projectId, db)!
    }
    db.query(
      `INSERT INTO watchers(project_id, enabled, interval_seconds, debounce_seconds, cooldown_seconds, max_paths, created_at, updated_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
    ).run(
      input.projectId,
      input.enabled ? 1 : 0,
      input.intervalSeconds ?? 10,
      input.debounceSeconds ?? 10,
      input.cooldownSeconds ?? 60,
      input.maxPaths ?? 10,
      timestamp,
      timestamp,
    )
    return getWatcher(input.projectId, db)!
  })
  return run()
}

export function getWatcher(projectId: string, db: Database = getDb()): WatcherRow | null {
  const row = db.query("SELECT * FROM watchers WHERE project_id = ?").get(projectId) as WatcherRecord | null
  return row ? toWatcherRow(row) : null
}

export function listWatchers(db: Database = getDb()): WatcherRow[] {
  const rows = db
    .query("SELECT w.*, p.name AS project_name FROM watchers w JOIN projects p ON p.id = w.project_id ORDER BY p.created_at ASC")
    .all() as (WatcherRecord & { project_name: string })[]
  return rows.map((row) => ({ ...toWatcherRow(row), projectName: row.project_name } as WatcherRow & { projectName: string }))
}

export function listEnabledWatchers(db: Database = getDb()): WatcherRow[] {
  const rows = db.query("SELECT * FROM watchers WHERE enabled = 1").all() as WatcherRecord[]
  return rows.map(toWatcherRow)
}

export function updateWatcherState(
  projectId: string,
  patch: Partial<{
    enabled: boolean
    observedFingerprint: string | null
    reportedFingerprint: string | null
    lastChangeAt: string | null
    lastDeliveredAt: string | null
  }>,
  db: Database = getDb(),
): WatcherRow | null {
  const sets: string[] = []
  const values: (string | number | null)[] = []
  if ("enabled" in patch && patch.enabled !== undefined) { sets.push("enabled = ?"); values.push(patch.enabled ? 1 : 0) }
  if ("observedFingerprint" in patch) { sets.push("observed_fingerprint = ?"); values.push(patch.observedFingerprint ?? null) }
  if ("reportedFingerprint" in patch) { sets.push("reported_fingerprint = ?"); values.push(patch.reportedFingerprint ?? null) }
  if ("lastChangeAt" in patch) { sets.push("last_change_at = ?"); values.push(patch.lastChangeAt ?? null) }
  if ("lastDeliveredAt" in patch) { sets.push("last_delivered_at = ?"); values.push(patch.lastDeliveredAt ?? null) }
  if (sets.length === 0) return getWatcher(projectId, db)
  sets.push("updated_at = ?")
  values.push(now())
  values.push(projectId)
  db.query(`UPDATE watchers SET ${sets.join(", ")} WHERE project_id = ?`).run(...values)
  return getWatcher(projectId, db)
}
