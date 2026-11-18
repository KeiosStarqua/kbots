# Backend

## Purpose

- Bun/TypeScript backend: the workflow daemon, OpenCode control utilities, and orchestration modules. See `../../ARCHITECTURE.md` for the system shape and `../../docs/kbots-ipc.md` for the local JSON-RPC contract the daemon serves.

## Ownership

- Owns `src/backend/` and its responsibility modules: `cli/`, `opencode/`, `orchestrator/`, `bots/`, `workflows/`, `adapters/`, `db/`. Each module has a `README.md` with its module-level contract.
- `daemon.ts` is the long-lived workflow daemon (`bun run daemon`); it owns workflow state, OpenCode sessions, the managed bot lifecycle, and side effects.
- `smoke.ts` is an embedded OpenCode host for SDK smoke checks — it is not the daemon and serves no clients.

## Local Contracts

- Use Bun and the exact dependency versions in `package.json`; Effect v4 (`effect` 4.1.4) for new application logic.
- `@opencode/sdk` for the embedded host, `@opencode/client` for the registered laptop service.
- Clients (CLI, GUI, TUI) never own bot scheduling or OpenCode sessions; all daemon-owned operations go through the IPC contract in `../../docs/kbots-ipc.md` (v2 task-centric methods: schema v5, per-task CEO session pool, `tasks.message`/`tasks.brief`/`tasks.inbox`/`tasks.update`, auto-dispatch on `tasks.create`).
- Read saved provider connections through OpenCode; never print credential values or auth headers.
- Session commands default to the current directory; pass `--directory` to select another project. Prompting and interrupting sessions are explicit actions. Do not delete sessions. Interrupting a session does not stop a recurring schedule attached to it.
- Every configured project uses `.agents/bot/{ceo,git,ops}`; the `.agents/bot/kbots.json` manifest generates per-role `opencode.jsonc` files. Preserve existing configs by default; regenerate only with `kbots project configure --overwrite`. Never copy project-specific bot instructions, plugins, or local state between repos.

## Work Guidance

- Keep modules separated by responsibility. Move transitional entry points (`smoke.ts`, `cli/control.ts`, `cli/start-sessions.ts`) into the responsibility modules as the orchestrator is built.
- `docs/opencode-control.md` owns embedded SDK setup, local service client, provider behavior, and session commands. `docs/kbots-ipc.md` owns the daemon transport contract. Keep both current when behavior changes.
- `orchestrator/router.selfcheck.ts` is a self-check script, not a test suite.

## Verification

- `bun run tsc --noEmit` (from repo root) after code changes.
- `bun run control --list-sessions` for read-only service verification.

## Child DOX Index

None — the responsibility modules (`cli/`, `opencode/`, `orchestrator/`, `bots/`, `workflows/`, `adapters/`, `db/`) are documented by their own `README.md` files; they are not durable enough yet to warrant nested AGENTS.md files.
