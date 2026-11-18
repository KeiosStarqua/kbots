# Backend layout

The Bun process owns workflow state, OpenCode sessions, bot lifecycles, and
side effects. The Slint desktop client lives in `../../gui/control-center/` and
will connect through a local IPC API; it must remain safe to close while jobs
continue running.

- `cli/`: command entry points and user-facing command parsing.
- `opencode/`: OpenCode service and session adapters.
- `orchestrator/`: scheduling, state transitions, event ingestion, and recovery.
- `bots/`: CEO, GIT, OPS, and coding-agent policies.
- `workflows/`: workflow definitions and task contracts.
- `adapters/`: Git, Herdr, and agmsg integrations.
- `db/`: SQLite schema, migrations, and repositories.

Current root package scripts use `smoke.ts` and `cli/control.ts` as transitional
entry points. Move their implementation into the responsibility modules as the
orchestrator is built.
