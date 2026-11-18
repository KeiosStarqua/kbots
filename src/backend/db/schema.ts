export const SCHEMA_VERSION = 5

// v1 → v2 delta: the managed agent registry. Applied to databases already at
// user_version 1; fresh databases run SCHEMA_DDL which embeds everything.
export const AGENTS_DDL = `
CREATE TABLE IF NOT EXISTS agents(
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK(role IN ('ceo','git','ops','worker')),
  mode TEXT NOT NULL CHECK(mode IN ('headless','terminal')),
  directory TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('starting','running','stopping','stopped','error')),
  session_id TEXT,
  hc_address TEXT,
  name TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  stopped_at TEXT,
  last_error TEXT
);

CREATE INDEX IF NOT EXISTS idx_agents_project ON agents(project_id);
CREATE INDEX IF NOT EXISTS idx_agents_status ON agents(status);
CREATE UNIQUE INDEX IF NOT EXISTS idx_agents_active_project_role
  ON agents(project_id, role)
  WHERE status IN ('starting','running','stopping') AND role != 'ceo';
`

// v2 → v3 delta: per-project file-change watchers (git working tree).
export const WATCHERS_DDL = `
CREATE TABLE IF NOT EXISTS watchers(
  project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
  enabled INTEGER NOT NULL DEFAULT 1,
  interval_seconds INTEGER NOT NULL DEFAULT 10,
  debounce_seconds INTEGER NOT NULL DEFAULT 10,
  cooldown_seconds INTEGER NOT NULL DEFAULT 60,
  max_paths INTEGER NOT NULL DEFAULT 10,
  observed_fingerprint TEXT,
  reported_fingerprint TEXT,
  last_change_at TEXT,
  last_delivered_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
`

// v3 → v4 delta: task-centric mission control.
// Rebuilds `tasks` (SQLite cannot alter a CHECK) preserving ids and remapping
// legacy statuses: planning → queued, in_progress → running, completed → done,
// canceled unchanged. The rebuild also adds the new `goal`/`detail` columns.
// Creates task_agents / task_messages / task_briefs and narrows
// idx_agents_active_project_role to role != 'ceo' (many active CEOs per
// project allowed; git/ops unchanged).
//
// The caller wraps this in `PRAGMA foreign_keys = OFF/ON` because approvals
// references tasks(id) and DROP TABLE would implicitly delete referenced rows.
export const TASKS_V4_DDL = `
DROP TABLE IF EXISTS tasks_v4;
CREATE TABLE tasks_v4(
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('queued','researching','planning','delegating','running','review','blocked','done','failed','canceled')),
  project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  canceled_at TEXT,
  goal TEXT,
  detail TEXT
);
INSERT INTO tasks_v4(id, title, status, project_id, created_at, updated_at, canceled_at, goal, detail)
  SELECT id, title,
    CASE status
      WHEN 'planning' THEN 'queued'
      WHEN 'in_progress' THEN 'running'
      WHEN 'completed' THEN 'done'
      ELSE status
    END,
    project_id, created_at, updated_at, canceled_at, NULL, NULL
  FROM tasks;
DROP TABLE tasks;
ALTER TABLE tasks_v4 RENAME TO tasks;
CREATE INDEX IF NOT EXISTS idx_tasks_project_id ON tasks(project_id);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);

CREATE TABLE IF NOT EXISTS task_agents(
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK(role IN ('ceo','worker','git','ops')),
  created_at TEXT NOT NULL,
  PRIMARY KEY (task_id, agent_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_task_agents_ceo ON task_agents(task_id) WHERE role = 'ceo';
CREATE INDEX IF NOT EXISTS idx_task_agents_agent ON task_agents(agent_id);

CREATE TABLE IF NOT EXISTS task_messages(
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  ts TEXT NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('user','system')),
  text TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('queued','delivered','canceled')),
  delivered_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_task_messages_task ON task_messages(task_id, status);

CREATE TABLE IF NOT EXISTS task_briefs(
  task_id TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
  summary TEXT,
  decisions TEXT,      -- newline-separated, newest last
  blockers TEXT,       -- newline-separated; empty when none
  next_step TEXT,
  updated_at TEXT NOT NULL
);

DROP INDEX IF EXISTS idx_agents_active_project_role;
CREATE UNIQUE INDEX IF NOT EXISTS idx_agents_active_project_role
  ON agents(project_id, role)
  WHERE status IN ('starting','running','stopping') AND role != 'ceo';
`

// v4 → v5 delta: additive columns only. No table rebuilds.
//   agents.last_outcome / last_outcome_at — outcome of the last probed run.
//   task_messages.attempts — delivery attempts, bounded by the daemon.
// Fresh SCHEMA_DDL embeds these directly.
export const V5_DDL = `
ALTER TABLE agents ADD COLUMN last_outcome TEXT;
ALTER TABLE agents ADD COLUMN last_outcome_at TEXT;
ALTER TABLE task_messages ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0;
`

export const SCHEMA_DDL = `
CREATE TABLE IF NOT EXISTS projects(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  path TEXT NOT NULL UNIQUE,
  bot_roles TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS tasks(
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('queued','researching','planning','delegating','running','review','blocked','done','failed','canceled')),
  project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  canceled_at TEXT,
  goal TEXT,
  detail TEXT
);

CREATE TABLE IF NOT EXISTS approvals(
  id TEXT PRIMARY KEY,
  operation TEXT NOT NULL,
  detail TEXT,
  project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
  task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
  status TEXT NOT NULL CHECK(status IN ('pending','approved')),
  created_at TEXT NOT NULL,
  decided_at TEXT
);

CREATE TABLE IF NOT EXISTS events(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  ts TEXT NOT NULL,
  kind TEXT NOT NULL,
  message TEXT NOT NULL,
  data TEXT
);

CREATE TABLE IF NOT EXISTS meta(
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_tasks_project_id ON tasks(project_id);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
CREATE INDEX IF NOT EXISTS idx_approvals_status ON approvals(status);

CREATE TABLE IF NOT EXISTS agents(
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK(role IN ('ceo','git','ops','worker')),
  mode TEXT NOT NULL CHECK(mode IN ('headless','terminal')),
  directory TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('starting','running','stopping','stopped','error')),
  session_id TEXT,
  hc_address TEXT,
  name TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  stopped_at TEXT,
  last_error TEXT,
  last_outcome TEXT,
  last_outcome_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_agents_project ON agents(project_id);
CREATE INDEX IF NOT EXISTS idx_agents_status ON agents(status);
CREATE UNIQUE INDEX IF NOT EXISTS idx_agents_active_project_role
  ON agents(project_id, role)
  WHERE status IN ('starting','running','stopping') AND role != 'ceo';

CREATE TABLE IF NOT EXISTS watchers(
  project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
  enabled INTEGER NOT NULL DEFAULT 1,
  interval_seconds INTEGER NOT NULL DEFAULT 10,
  debounce_seconds INTEGER NOT NULL DEFAULT 10,
  cooldown_seconds INTEGER NOT NULL DEFAULT 60,
  max_paths INTEGER NOT NULL DEFAULT 10,
  observed_fingerprint TEXT,
  reported_fingerprint TEXT,
  last_change_at TEXT,
  last_delivered_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS task_agents(
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK(role IN ('ceo','worker','git','ops')),
  created_at TEXT NOT NULL,
  PRIMARY KEY (task_id, agent_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_task_agents_ceo ON task_agents(task_id) WHERE role = 'ceo';
CREATE INDEX IF NOT EXISTS idx_task_agents_agent ON task_agents(agent_id);

CREATE TABLE IF NOT EXISTS task_messages(
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  ts TEXT NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('user','system')),
  text TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('queued','delivered','canceled')),
  delivered_at TEXT,
  attempts INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_task_messages_task ON task_messages(task_id, status);

CREATE TABLE IF NOT EXISTS task_briefs(
  task_id TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
  summary TEXT,
  decisions TEXT,      -- newline-separated, newest last
  blockers TEXT,       -- newline-separated; empty when none
  next_step TEXT,
  updated_at TEXT NOT NULL
);
`
