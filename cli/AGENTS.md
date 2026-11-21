# Automation CLI

## Purpose

- `kbots` is a Rust command-line client for automation. `kb` is a short alias `install.sh` links to it (`~/.local/bin/kb` → `kbots`). It exposes stable subcommands, then sends daemon-owned operations over a local Unix domain socket. It does not implement orchestration or bypass daemon-side authorization and idempotency checks. When stdout is a terminal, output is a human-readable rendering (tables and summaries); when stdout is piped — or `--json` is passed — output is pretty-printed JSON, so scripts keep a stable machine-readable contract.

## Ownership

- Owns this directory: the Cargo package (binary `kbots`), `src/`, and `templates/bots/{ceo,git,ops,router}/AGENTS.md` starter files used when seeding `.agents/bot/` role directories. The `router` starter seeds the embedded Codex task-router folder and is never added to the manifest or OpenCode configs.
- Does not own orchestration logic, workflow state, or session scheduling — those live in the Bun backend (`../src/backend/`).

## Local Contracts

- Daemon-owned operations go over the local Unix socket (JSON-RPC 2.0); the contract is `../docs/kbots-ipc.md`. The CLI is a client, not a bypass.
- `project add`, `project configure`, and `project sync` are local/direct OpenCode operations; everything else (tasks, agents, approvals) is daemon-owned.
- `project add` / `project configure` adopt a project that already ships per-role `opencode.jsonc`: the manifest `.agents/bot/kbots.json` is derived from those configs (JSONC-aware, `$schema` dropped, roles without a config get defaults) and the config files stay byte-identical unless `--overwrite`. The scaffold stamp `.agents/bot/.scaffold.json` is machine-local and self-ignored via `.agents/bot/.gitignore`.
- `project configure` expands `bots.<role>.opencode.mcp.disable` into `mcp.servers.<name>.disabled` entries by copying the definition from the global OpenCode config (`type`, `command`, `cwd`, `url`, `timeout` only — never `environment`, `headers`, or `oauth`). Keep it that way: a tracked project config must not carry secrets.
- `project sync` refreshes the embedded starter `AGENTS.md` files in an existing project: missing files are created, template-identical files are left alone, and files recorded as untouched in `.agents/bot/.scaffold.json` are upgraded to the new template. Customized files are kept unless `--force` is passed (each is backed up first as `AGENTS.md.bak[.N]`). `project sync --check` reports stale/customized without writing. The stamp records the embedded `KBOTS_VERSION` (set by `./install.sh`) plus the template and per-file hashes; `prj ls` surfaces it. The project manifest and OpenCode configs are not touched.
- `tasks update` moves a task only to `review`, `blocked`, `done`, `failed`, or `canceled`; the daemon validates the status and rejects terminal-state changes.
- `tasks message TASK-ID TEXT` delivers a message to the task's CEO (or queues it when the CEO is busy); `tasks brief` and `tasks inbox` render daemon-generated summaries — never compute them client-side.
- Stdout contract: human-readable on a TTY, pretty JSON when piped or `--json`. Keep both renderings in sync when adding subcommands.

## Work Guidance

- Command examples and subcommand behavior live in `README.md`; keep it updated when subcommands change.
- Starter templates in `templates/bots/` seed new projects; edit them intentionally and keep them consistent with the manifest flow in the backend (`.agents/bot/kbots.json` → per-role `opencode.jsonc`).
- Alias handling (`kb`, `ag`, `ls`, `prj`, `-p`) lives in argument parsing; preserve existing aliases — scripts depend on them.

## Verification

- `cargo check` (or `cargo build`) in this directory.
- `./install.sh` from the repo root builds and installs the CLI alongside the other clients and deploys the daemon on non-Nix hosts; on Nix-managed hosts the flake package provides `kbots` and the `kb` alias, and `home-manager switch` installs it.

## Child DOX Index

None.
