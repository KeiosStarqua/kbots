export const SCHEMA_VERSION = 2

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
  WHERE status IN ('starting','running','stopping');
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
  status TEXT NOT NULL CHECK(status IN ('planning','in_progress','completed','canceled')),
  project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  canceled_at TEXT
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
  last_error TEXT
);

CREATE INDEX IF NOT EXISTS idx_agents_project ON agents(project_id);
CREATE INDEX IF NOT EXISTS idx_agents_status ON agents(status);
CREATE UNIQUE INDEX IF NOT EXISTS idx_agents_active_project_role
  ON agents(project_id, role)
  WHERE status IN ('starting','running','stopping');
`
