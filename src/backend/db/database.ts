import { Database } from "bun:sqlite"
import { mkdirSync } from "node:fs"
import { homedir } from "node:os"
import { dirname, join } from "node:path"
import { AGENTS_DDL, SCHEMA_DDL, SCHEMA_VERSION, TASKS_V4_DDL, WATCHERS_DDL } from "./schema.ts"

export function defaultDatabasePath(): string {
  const stateRoot = process.env.XDG_STATE_HOME ?? join(homedir(), ".local/state")
  return join(stateRoot, "kbots", "kbots.db")
}

export function openDatabase(path: string = defaultDatabasePath()): Database {
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 })
  const db = new Database(path)
  db.exec("PRAGMA journal_mode = WAL")
  db.exec("PRAGMA foreign_keys = ON")
  const row = db.query("PRAGMA user_version").get() as { user_version: number } | null
  const current = row?.user_version ?? 0
  if (current === 0) {
    db.exec(SCHEMA_DDL)
  } else {
    if (current === 1) db.exec(AGENTS_DDL)
    if (current <= 2) db.exec(WATCHERS_DDL)
    if (current <= 3) {
      // v4 rebuilds `tasks`; approvals references tasks(id), so the rebuild
      // must run with FK enforcement off (restored right after).
      db.exec("PRAGMA foreign_keys = OFF")
      try {
        db.exec(TASKS_V4_DDL)
      } finally {
        db.exec("PRAGMA foreign_keys = ON")
      }
    }
  }
  if (current < SCHEMA_VERSION) {
    db.exec(`PRAGMA user_version = ${SCHEMA_VERSION}`)
  }
  return db
}
