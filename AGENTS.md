# kbots

## Purpose

- Local Bun tools for OpenCode SDK smoke checks and laptop-service control.

## Ownership

- This folder owns its package, lockfile, scripts, and `docs/` reference material.
- Keep the Bun/TypeScript orchestration backend in `src/backend/`, the Slint
  desktop client in `gui/control-center/`, the Ratatui terminal client in
  `tui/control-center/`, and the automation CLI in `cli/`. GUI, TUI,
  and CLI talk to the daemon over a local IPC boundary; clients do not own bot
  scheduling or OpenCode sessions.
- Keep sample UI state clearly marked until connected to the backend API. The
  TUI must restore terminal state on normal exit and on panics.
- After completing application code changes, run `./install.sh` to synchronize
  Bun dependencies, type-check, build the Rust clients, and install their
  binaries. Use `KBOTS_INSTALL_DIR` when the default `~/.local/bin` is not the
  intended destination.
- Keep backend modules separated by responsibility: `cli/`, `opencode/`,
  `orchestrator/`, `bots/`, `workflows/`, `adapters/`, and `db/`.
- `film-production/.agents/bot/kir-auto/` owns the scheduled bot instructions and job log.

## Local Contracts

- Use Bun and the exact dependency versions in `package.json`.
- Use Effect v4 (`effect`) for new Bun/TypeScript application logic, especially typed failures,
  resource management, and asynchronous workflows. Import from `effect` and
  follow the v4 documentation; avoid introducing new ad hoc promise/error
  orchestration when Effect fits the operation.
- Use `@opencode/sdk` for an embedded OpenCode host and `@opencode/client` for the registered
  laptop service.
- Every configured project uses `.agents/bot/{ceo,git,ops}`. `kbots project add`
  creates missing role directories and starter `AGENTS.md` files. The editable
  `.agents/bot/kbots.json` manifest generates per-role `opencode.jsonc` files;
  preserve existing configs by default and regenerate them only with the
  explicit `kbots project configure --overwrite` command. Never copy
  project-specific bot instructions, plugins, or local state between repos.
- Read saved provider connections through OpenCode; never print credential values or auth headers.
- Session commands default to `KIR_AUTO_DIRECTORY` or the sibling film-production checkout.
- Prompting and interrupting sessions are explicit actions. Do not delete sessions.
- Interrupting a session does not stop a recurring schedule attached to it.

## Work Guidance

- Keep session inspection read-only and scope existing-session actions to the selected directory.
- Keep operational and API details in `docs/opencode-control.md`.
- Keep the GUI/TUI/CLI daemon transport contract in `docs/kbots-ipc.md`.
- After completing and verifying a requested code change, stage only that task's
  files, create one focused atomic commit, and push it to the current branch's
  configured upstream without asking for confirmation. Never include unrelated
  pre-existing changes in the commit.

## Verification

- Run `bun run tsc --noEmit` after code changes.
- Use `bun run control --list-sessions` for read-only service verification.

## Child DOX Index

- `docs/opencode-control.md`: embedded SDK setup, local service client, provider behavior, and
  session commands.
- `docs/kbots-ipc.md`: local JSON-RPC protocol for the daemon clients.
- `cli/README.md`: automation CLI behavior and command examples.
- `gui/README.md` and `tui/README.md`: UI client ownership and run commands.
