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

export type TaskStatus = "planning" | "in_progress" | "completed" | "canceled"

export interface TaskRow {
  taskId: string
  title: string
  status: TaskStatus
  projectId: string | null
  projectPath: string | null
  createdAt: string
  updatedAt: string
  canceledAt: string | null
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

function getDb(): Database {
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

const TASK_STATUSES: readonly TaskStatus[] = ["planning", "in_progress", "completed", "canceled"]

function toTaskStatus(value: string): TaskStatus {
  return (TASK_STATUSES as readonly string[]).includes(value) ? (value as TaskStatus) : "planning"
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
  input: { title: string; projectId?: string | null },
  db: Database = getDb(),
): TaskRow {
  const run = db.transaction((): TaskRow => {
    const timestamp = now()
    const id = nextId("TASK", db)
    db.query(
      "INSERT INTO tasks(id, title, status, project_id, created_at, updated_at, canceled_at) VALUES (?, ?, 'planning', ?, ?, ?, NULL)",
    ).run(id, input.title, input.projectId ?? null, timestamp, timestamp)
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
  filter?: { projectId?: string },
  db: Database = getDb(),
): TaskRow[] {
  if (filter?.projectId !== undefined) {
    const rows = db
      .query(`${TASK_SELECT} WHERE tasks.project_id = ? ORDER BY tasks.created_at DESC, tasks.id DESC`)
      .all(filter.projectId) as TaskRecord[]
    return rows.map(toTaskRow)
  }
  const rows = db
    .query(`${TASK_SELECT} ORDER BY tasks.created_at DESC, tasks.id DESC`)
    .all() as TaskRecord[]
  return rows.map(toTaskRow)
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
