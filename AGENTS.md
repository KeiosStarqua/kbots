# DOX framework

- DOX is highly performant AGENTS.md hierarchy installed here
- Agent must follow DOX instructions across any edits

## Core Contract

- AGENTS.md files are binding work contracts for their subtrees
- Work products, source materials, instructions, records, assets, and durable docs must stay understandable from the nearest applicable AGENTS.md plus every parent AGENTS.md above it

## Read Before Editing

1. Read the root AGENTS.md
2. Identify every file or folder you expect to touch
3. Walk from the repository root to each target path
4. Read every AGENTS.md found along each route
5. If a parent AGENTS.md lists a child AGENTS.md whose scope contains the path, read that child and continue from there
6. Use the nearest AGENTS.md as the local contract and parent docs for repo-wide rules
7. If docs conflict, the closer doc controls local work details, but no child doc may weaken DOX

Do not rely on memory. Re-read the applicable DOX chain in the current session before editing.

## Update After Editing

Every meaningful change requires a DOX pass before the task is done.

Update the closest owning AGENTS.md when a change affects:

- purpose, scope, ownership, or responsibilities
- durable structure, contracts, workflows, or operating rules
- required inputs, outputs, permissions, constraints, side effects, or artifacts
- user preferences about behavior, communication, process, organization, or quality
- AGENTS.md creation, deletion, move, rename, or index contents

Update parent docs when parent-level structure, ownership, workflow, or child index changes. Update child docs when parent changes alter local rules. Remove stale or contradictory text immediately. Small edits that do not change behavior or contracts may leave docs unchanged, but the DOX pass still must happen.

## Hierarchy

- Root AGENTS.md is the DOX rail: project-wide instructions, global preferences, durable workflow rules, and the top-level Child DOX Index
- Child AGENTS.md files own domain-specific instructions and their own Child DOX Index
- Each parent explains what its direct children cover and what stays owned by the parent
- The closer a doc is to the work, the more specific and practical it must be

## Child Doc Shape

- Create a child AGENTS.md when a folder becomes a durable boundary with its own purpose, rules, responsibilities, workflow, materials, or quality standards
- Work Guidance must reflect the current standards of the project or user instructions; if there are no specific standards or instructions yet, leave it empty
- Verification must reflect an existing check; if no verification framework exists yet, leave it empty and update it when one exists

Default section order:
- Purpose
- Ownership
- Local Contracts
- Work Guidance
- Verification
- Child DOX Index

## Style

- Keep docs concise, current, and operational
- Document stable contracts, not diary entries
- Put broad rules in parent docs and concrete details in child docs
- Prefer direct bullets with explicit names
- Do not duplicate rules across many files unless each scope needs a local version
- Delete stale notes instead of explaining history
- Trim obvious statements, repeated rules, misplaced detail, and warnings for risks that no longer exist

## Closeout

1. Re-check changed paths against the DOX chain
2. Update nearest owning docs and any affected parents or children
3. Refresh every affected Child DOX Index
4. Remove stale or contradictory text
5. Run existing verification when relevant
6. Report any docs intentionally left unchanged and why

## User Preferences

When the user requests a durable behavior change, record it here or in the relevant child AGENTS.md

- Deploy is one command: `kbdeploy "what changed"`. Do not give or run the underlying git / tag / `nix flake update` / `home-manager switch` steps unless `kbdeploy` itself is broken.

### Purpose

- Local Bun tools for OpenCode SDK smoke checks and laptop-service control.
- `kbots` is a local coding workflow orchestrator: one long-lived Bun daemon owns project/workflow state and OpenCode sessions; Rust CLI, Slint desktop, and Ratatui terminal clients connect to it. See `ARCHITECTURE.md` for the system shape and what is implemented vs planned.

### Ownership

- This repo owns its package, lockfile, scripts, and `docs/` reference material.
- Keep the Bun/TypeScript orchestration backend in `src/backend/`, the Slint desktop client in `gui/`, the Ratatui terminal client in `tui/control-center/`, and the automation CLI in `cli/`. GUI, TUI, and CLI talk to the daemon over a local IPC boundary; clients do not own bot scheduling or OpenCode sessions.
- Keep sample UI state clearly marked until connected to the backend API. The TUI must restore terminal state on normal exit and on panics.
- **One command to deploy on Nix-managed hosts: `kbdeploy "what changed"`.** `deploy/kbdeploy.sh` is packaged as `bin/kbdeploy`: it commits the checkout, creates a local `deploy/YYYY.MM.DD.n` tag and pushes commit + tag, runs `nix flake update kbots`, runs `home-manager switch --flake ~/.config/home-manager#$(id -un)`, and shows `kb status`. After code changes, use only this command — never list or run the underlying steps. `./install.sh` is the equivalent one command for non-Nix hosts: it synchronizes Bun dependencies, type-checks, builds the Rust clients, installs their binaries, and deploys `deploy/kbots-daemon.service` as the `kbots-daemon` systemd user service (enable + restart) when a user systemd session is available. Clients must always be running freshly deployed code; use `KBOTS_SKIP_DAEMON_UNIT=1 ./install.sh` only when explicitly unwanted, and then restart the daemon yourself (`systemctl --user restart kbots-daemon`). Use `KBOTS_INSTALL_DIR` when the default `~/.local/bin` is not the intended destination. Every `install.sh` deploy is tagged `deploy/YYYY.MM.DD.n` (CalVer, local) and stamped in `.deploy-version`; stable releases use SemVer `vX.Y.Z` tags. On Windows the one command is `powershell -ExecutionPolicy Bypass -File .\install.ps1`: per-user binaries in `%LOCALAPPDATA%\kbots\bin`, no WSL and no admin, TCP daemon managed by `kbots daemon start|stop|status`. See `docs/deployment.md`.
- `flake.nix` and the Home Manager module are part of this repo's install contract. Keep `bunDeps.outputHash` in sync with `bun.lock` (a mismatch prints the expected hash), keep the daemon's version equal to `cli/Cargo.toml`, and never make the Nix install write to `~/.local/bin` or an unmanaged systemd unit.
- Keep backend modules separated by responsibility: `cli/`, `opencode/`, `orchestrator/`, `bots/`, `workflows/`, `adapters/`, and `db/` under `src/backend/`.
- Keep client-shared IPC in `ipc/` (Rust crate `kbots-ipc`): Unix sockets on Unix, authenticated TCP on Windows/remote. CLI, TUI, and GUI use it for daemon discovery and token auth; never reimplement transport in a client.
- The Rust CLI directory is `cli/` (package `kbots-cli`, binary `kbots`, alias `kb`). The GUI lives in `gui/`, not `gui/control-center/`.

### Local Contracts

- Use Bun and the exact dependency versions in `package.json`.
- Use Effect v4 (`effect`) for new Bun/TypeScript application logic, especially typed failures, resource management, and asynchronous workflows. Import from `effect` and follow the v4 documentation; avoid introducing new ad hoc promise/error orchestration when Effect fits the operation.
- Use `@opencode/sdk` for an embedded OpenCode host and `@opencode/client` for the registered laptop service.
- Every configured project uses `.agents/bot/{ceo,git,ops}` plus a `.agents/bot/router/` folder with the embedded Codex task-router instructions (not an OpenCode role; Codex only). `kbots project add` / `kbots project sync` create the starter `AGENTS.md` files for all four. The editable `.agents/bot/kbots.json` manifest generates per-role `opencode.jsonc` files; preserve existing configs by default and regenerate them only with the explicit `kbots project configure --overwrite` command. When a project already ships `opencode.jsonc` files without a manifest, `project add`/`configure` derive `kbots.json` from them (JSONC-aware, roles without a config get defaults) instead of refusing; the existing configs stay byte-identical. The scaffold stamp `.agents/bot/.scaffold.json` is machine-local and self-ignored via `.agents/bot/.gitignore`. Never copy project-specific bot instructions, plugins, or local state between repos.
- Read saved provider connections through OpenCode; never print credential values or auth headers.
- Session commands default to the current directory; pass `--directory` to select another project.
- Prompting and interrupting sessions are explicit actions. Do not delete sessions.
- Interrupting a session does not stop a recurring schedule attached to it.
- The daemon maintenance loop (60 s) reconciles agents, stops CEO terminals whose task already reached `done`/`failed`/`canceled` after a grace period, reclaims live kbots-labelled terminals that no agent row references any more (a stray must appear in two consecutive ticks before it is killed, so a respawn in progress is never caught), and resumes a CEO that crashed or exited while its task is still open (only tasks from the last `KBOTS_RECOVER_WINDOW_MS`, bounded by `KBOTS_RECOVER_MAX`/`KBOTS_RECOVER_COOLDOWN_MS`, never a row the user stopped with `agents stop`). Tasks older than that window are handled by the stale-task continuation step, which resumes their CEO on the same bounded budget instead of leaving them in `researching` forever. Every stop and resume is the `agents stop`/`agents start` path — the session is kept; respawns retire the previous terminal first. `KBOTS_REAP=off` / `KBOTS_ORPHAN_REAP=off` / `KBOTS_RECOVER=off` / `KBOTS_STALE_CONTINUE=off` disable the respective steps.
- Every managed agent carries a derived `report` (activity, health level/code/message/hint, bound task) from `src/backend/bots/health.ts`. Clients render it; never re-implement the health rules in the CLI, TUI, or GUI.
- The daemon's file-change watcher auto-starts (or resumes) a project's git agent when a watched tree has changes, unless an operator explicitly stopped it, and prompts it with the current branch. The git bot owns the primary branch (`main`): on `main` it commits the whole change set (including other agents' work) and pushes without another approval; other branches are inspect-only. Deployment stays with the repo owner via `kbdeploy`, never with the git bot.

### Work Guidance

- Keep session inspection read-only and scope existing-session actions to the selected directory.
- Fixes belong in kbots code, not in project repos. When a project's scaffolding, manifest, config, or daemon/CLI behavior is wrong, change the kbots code (CLI, daemon, templates) with a verification and let the fixed code resolve it — do not hand-edit downstream repos, and do not present "run this manually in each project" as the fix. Operate on projects only through kbots commands (`kb project add|configure|sync`, `kb sessions start`, `kb agents …`), and keep the resolution idempotent so every registered project converges by re-running the command.
- Keep operational and API details in `docs/opencode-control.md`.
- Keep the GUI/TUI/CLI daemon transport contract in `docs/kbots-ipc.md`.
- After completing and verifying a requested code change, stage only that task's files, create one focused atomic commit, and push it to the current branch's configured upstream without asking for confirmation. Never include unrelated pre-existing changes in the commit.
- Changes under `.agents/bot/` in this repo are the repo's own bot configuration; do not treat them as templates for other projects and do not copy them between repos.

### Verification

- Run `bun run tsc --noEmit` after code changes.
- Use `bun run control --list-sessions` for read-only service verification.
- `nix build .#` is the packaging check (all four binaries, `kbdeploy`, desktop entry, `kb` alias); `nix develop` provides the Bun/Rust/Slint toolchain.
- `kbdeploy "what changed"` is the one deploy command on Nix-managed hosts; `./install.sh` on Unix hosts and `powershell -ExecutionPolicy Bypass -File .\install.ps1` on Windows are the one-command deploys elsewhere.

### Architecture Principles

Code follows **Clean Architecture**: dependencies point inward (entities → use cases → adapters → frameworks). Business rules never depend on UI, DB, or framework details.

#### SOLID

- **S**: One reason to change per module
- **O**: Extend behavior with new types, not edits to stable code
- **L**: Subtypes must honor base contracts
- **I**: Small, role-specific interfaces
- **D**: Inner layers depend on abstractions, not concrete frameworks

#### Frontend: feature layer

This repo has no web frontend. Native clients (`gui/`, `tui/control-center/`) stay thin and delegate to the daemon. When UI code grows, organize by feature under `features/<name>/` (components, hooks, api, types). Features may import `shared/`; `shared/` never imports features. Export only the public surface via `index.ts`.

#### Deep modules

Prefer modules with **simple interfaces and substantial hidden complexity**. Avoid shallow pass-through wrappers. Split only when a module hides real complexity — not to hit arbitrary file-size limits.

#### Design patterns

Use GoF patterns when they match a recurring problem — not for decoration. Catalog: https://refactoring.guru/design-patterns/catalog

#### TypeScript: Effect

The codebase uses TypeScript (`tsconfig.json`, `.ts` sources, `effect` 4.1.4). Use [Effect](https://effect.website) for TypeScript code: model side effects, errors, and dependencies with `Effect` instead of ad-hoc `Promise`/`try-catch`/singletons. Read [Why Effect](https://effect.website/docs/v4/getting-started/why-effect) for the rationale and follow [Installation](https://effect.website/docs/v4/getting-started/installation) for setup. Follow the Effect v4 docs, not older v3 patterns; align with the installed `effect` 4.x API.

#### UI design: Apple HIG

Design every UI against the [Apple Human Interface Guidelines](https://developer.apple.com/design/human-interface-guidelines/) and its eight [design principles](https://developer.apple.com/design/human-interface-guidelines/design-principles): Purpose, Agency, Responsibility, Familiarity, Flexibility, Simplicity, Craft, Delight. Get people straight to the task, make actions reversible, give clear feedback, explain every permission and data use, keep visuals and interactions consistent, and treat accessibility as a requirement from the start. Before building or reviewing a screen, read the matching [Foundations](https://developer.apple.com/design/human-interface-guidelines/foundations), [Patterns](https://developer.apple.com/design/human-interface-guidelines/patterns), and [Components](https://developer.apple.com/design/human-interface-guidelines/components) pages and follow their best practices. On Apple platforms, also follow each page's platform considerations and prefer system components. On other platforms, do not copy Apple-only conventions that conflict with the host platform. Slint and Ratatui UIs follow these principles where applicable to native desktop/terminal conventions. Icons stay on Phosphor (below).

#### UI icons

Use [Phosphor Icons](https://github.com/phosphor-icons/homepage#phosphor-icons) for every icon in the UI. Pick the official package for the stack (React: `@phosphor-icons/react`, Vue: `@phosphor-icons/vue`, web: `@phosphor-icons/web`, or the matching port). Do not mix in another icon set.

### Live progress reporting

Whenever an agent works on a task, it keeps the task's issue on the repo's issue tracker current **while it runs**, not only when it finishes. The tracker is the one in `.cursor/loop.jsonc` (`issueTracker`: `linear` or `itsaplan`; see `.cursor/rules/LOOP.mdc` for projects, statuses, and the default project).

- **Find the issue first.** Work on an existing issue, or create one in the default project before starting. Keep its status truthful (`In Progress` while working, per `LOOP.mdc`).
- **Write to the tracker with the tracker's tools.** Linear: `save_comment`. itsaplan: `add_comment` (edit a previous one with `update_comment`). Edit a single running "Agent status" comment in place when the tool supports it; post a new comment for milestones, blockers, and completion so the owner is notified.
- **Report at these moments:** (1) task started and the plan in one or two lines; (2) each phase change (plan → implement → verify → PR); (3) before every long command; (4) while a long command runs; (5) after every long command; (6) when blocked, failing, or waiting on the owner; (7) done, with PR/commit links and the result.
- **Long commands** (builds, test suites, installs, deploys, migrations, generation jobs: anything expected to exceed ~30 seconds):
  - Before: comment the exact command, why, and the expected duration.
  - Run it with a timeout and non-interactively (`timeout 300 … < /dev/null`), or in the background with output redirected to a log file. Never leave a foreground or watch-mode process hanging.
  - During: update the status at least every ~5 minutes (or per phase) with elapsed time and the log tail. Never go silent.
  - After: comment exit code, duration, and the key lines of output. On failure, quote the error and state the next step.
- **Keep comments short and safe:** state, evidence, next step. Never post secrets, tokens, or full logs; link or summarize instead.
- If the tracker is unreachable, say so in the chat, keep working, and post the missed updates as soon as it is back.

## Child DOX Index

- `src/backend/AGENTS.md`: Bun/TypeScript backend — daemon, OpenCode control utilities, orchestration modules (`cli/`, `opencode/`, `orchestrator/`, `bots/`, `workflows/`, `adapters/`, `db/`).
- `ipc/AGENTS.md`: shared Rust IPC crate (`kbots-ipc`) — Unix/TCP discovery, auth token, JSON-RPC calls.
- `cli/AGENTS.md`: Rust automation CLI (`kbots`, alias `kb`) — thin daemon client, local project configuration, and daemon process lifecycle.
- `gui/AGENTS.md`: Slint native desktop operator-control client (shared IPC transport).
- `tui/AGENTS.md`: Ratatui/Crossterm native terminal client (`tui/control-center/`, shared IPC transport).

Reference material (not DOX children): `ARCHITECTURE.md` (system shape, implemented vs planned), `docs/opencode-control.md` (embedded SDK setup, service client, provider behavior, session commands), `docs/opencode-token-speed.md` (read-only measurement of tok/s, TTFT and build time from OpenCode's local SQLite DB), `docs/kbots-ipc.md` (local JSON-RPC protocol for the daemon clients), `docs/deployment.md` (deploy tags, daemon service), `docs/plans/` (design plans). Module-level detail lives in each backend folder's `README.md`, `cli/README.md`, `gui/README.md`, and `tui/README.md`.
