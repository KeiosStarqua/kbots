# kbots Architecture

## Purpose and status

`kbots` is a local coding workflow orchestrator. Its intended runtime has one
long-lived Bun daemon that owns project/workflow state and OpenCode sessions
(with an auto-kick nudge loop for idle task CEOs that have not marked their
task done),
with Rust command-line, desktop, and terminal clients. The repository currently
contains the OpenCode smoke/control utilities, the v1 workflow daemon extended
with the v2 task-centric dispatch surface (`docs/kbots-ipc.md`, "v2 methods":
schema v4, per-task CEO session pool, `tasks.message`/`tasks.brief`/`tasks.inbox`/`tasks.update`,
auto-dispatch on `tasks.create`), project configuration and idle session setup,
and the Rust clients; scheduling, workflow state transitions beyond dispatch,
and full persistence repositories remain planned. The IPC contract in
`docs/kbots-ipc.md` is served by the daemon. The terminal UI connects to the
daemon and renders a live task-centric dashboard; the Slint GUI also connects.

This distinction matters when reading the design below: **implemented** means
the code exists in this repository; **planned** means the responsibility or
contract is documented but still needs its owning backend implementation.

## System shape

```text
                    ┌───────────────────────────────┐
                    │ Local project directories     │
                    │ .agents/bot/{ceo,git,ops,audit,router}
                    │ kbots.json + OpenCode configs  │
                    └──────────────┬────────────────┘
                                   │ project setup / session start
                                   ▼
┌────────────────┐  Unix socket / TCP    ┌──────────────────────────────┐
│ Rust CLI       │ ─ ─ JSON-RPC 2.0 ─ ─▶│ Bun workflow daemon          │
│ Slint GUI      │                       │ (v1 minimal)                  │
│ Ratatui TUI    │                       │ orchestrator, workflows, DB, │
└────────────────┘                       │ bot policy, adapters          │
                                         └─────────────┬────────────────┘
                                                       │ OpenCode client
                                                       ▼
                                         ┌──────────────────────────────┐
                                         │ Local OpenCode service       │
                                         │ sessions, models, providers  │
                                         └──────────────────────────────┘

Separate utility path: `smoke` embeds an OpenCode host through `@opencode/sdk`.
It is not the workflow daemon and does not serve the clients.
```

The daemon interface is a Unix socket on Unix hosts and authenticated TCP on
Windows (`127.0.0.1:7878`) or with `--tcp`/`KBOTS_TCP`; the Rust clients share
discovery/auth through `ipc/`. Until the daemon is running, daemon-oriented CLI
calls fail with a connection error. `project configure` and `sessions start`
are local/direct OpenCode operations and work without that daemon.

## Repository ownership

| Path | Responsibility | State |
| --- | --- | --- |
| `src/backend/cli/` | Transitional Bun command entry points for OpenCode control and bot session startup | Implemented |
| `src/backend/opencode/` | Shared OpenCode service/session adapter | Planned module boundary; behavior currently lives in CLI scripts |
| `src/backend/orchestrator/` | v1 task/project/approval/system services; v2 task dispatch (`orchestrator/dispatch.ts`: auto-dispatch, CEO session pool, idempotent via `task_agents` unique CEO link, per-project cap `KBOTS_CEO_CAP`) and message delivery (`orchestrator/delivery.ts`); file-change watcher (`orchestrator/watchers.ts`) prompts the managed git agent on working-tree deltas with debounce + cooldown; maintenance (`orchestrator/maintenance.ts`) reclaims kbots-labelled terminals left unreferenced by a respawn; scheduling and broader workflow engine still planned | v1 services + git watcher + v2 dispatch + terminal reaping implemented; scheduling/workflow engine still planned |
| `src/backend/workflows/` | Task stages, dependencies, acceptance criteria, retry and approval rules | Planned |
| `src/backend/bots/` | agents.list implemented (incl. managed `agents.start/show/stop/prompt/interrupt` lifecycle: exact session binding, terminal+headless modes, SQLite registry, boot reconciliation); CEO planning/review, GIT and OPS work contracts, coding worker policies | Lifecycle implemented; work contracts still planned |
| `src/backend/adapters/` | Herdr adapter (read-only `hc ls`) implemented; Git and optional agmsg integrations | Herdr implemented; Git/agmsg planned |
| `src/backend/db/` | SQLite schema, migrations, repositories, transaction boundaries | Implemented minimal persistence (WAL) |
| `src/backend/daemon.ts` | Bun daemon entry point: newline-delimited JSON-RPC over Unix sockets (Unix) or authenticated TCP (Windows/remote), SQLite-backed; `adapters/transport.ts` owns the TCP address and token | Implemented (v1 minimal); systemd user service by install.sh on Unix, detached process via `kbots daemon start` on Windows |
| `ipc/` | Rust `kbots-ipc`: shared endpoint discovery, Unix/TCP streams, token auth, JSON-RPC framing | Implemented; consumed by CLI, GUI and TUI |
| `cli/` | Rust `kbots` automation client, local project scaffolding/configuration, UI launch, daemon lifecycle, remote-host config | Implemented |
| `gui/` | Native Slint operator client | UI and RPC calls implemented; connects to the v1 daemon |
| `tui/control-center/` | Native Ratatui dashboard; v1+v2 daemon client (live tasks/agents/approvals/events, task composer, `/brief`, `/inbox`) | Implemented; connects to the daemon |
| `docs/` | Detailed OpenCode and IPC operational contracts | Implemented documentation |
| `install.sh` | Frozen Bun install, TypeScript check, Rust builds, binary and desktop launcher install (Unix) | Implemented |
| `install.ps1` | Native Windows per-user install: frozen Bun install, TypeScript check, Rust builds, PATH/Start Menu/daemon setup; no WSL or admin | Implemented |

Each backend module has a local README describing its ownership. When those
modules are implemented, keep policy in the orchestrator/workflow layers and
make adapters expose operations rather than decisions.

## Runtime components

### Bun backend and OpenCode paths

The root Bun package pins exact dependency versions in `package.json` and
`bun.lock`: `@opencode/sdk`, `@opencode/client`, `@opencode/plugin`, and Effect
v4. New application logic belongs in the responsibility modules above and
should use Effect for typed failures, resource management, and async workflows.

There are three distinct OpenCode paths:

1. **Embedded smoke check — `src/backend/smoke.ts`.** `bun run smoke` starts an
   embedded host with `@opencode/sdk`, registers a small plugin, checks provider
   and account-connection availability, creates a temporary session, prompts
   for a fixed response, waits up to two minutes, reads the assistant text,
   and removes that temporary session in `finally`. It uses the OpenCode CLI's
   database path by default (`opencode debug paths db`), with
   `OPENCODE_DB_PATH` as an override. `OPENCODE_MODEL` selects a
   `provider/model`; the default is `opencode/inkling-2-small-free`. It does not
   print provider credentials or database contents.
2. **Registered local service control — `src/backend/cli/control.ts`.**
   `bun run control` uses `Service.ensure()` and `Service.headers()` from
   `@opencode/client` to reuse or start the laptop OpenCode service. It reports
   service metadata and providers. Prompting is opt-in. Existing-session
   actions confirm the session belongs to the selected directory; listing and
   inspection are read-only. Interrupt stops the current run only and neither
   deletes the session nor disables a recurring schedule. Session commands
   default to the current directory, like the other control commands.
   Prompt waits are capped at two minutes.
3. **Per-project idle bot sessions — `src/backend/cli/start-sessions.ts`.**
   `kbots sessions start` reads the project's manifest, ensures the local
   OpenCode service, then creates or reuses one idle session per enabled role.
   Sessions run in each role's directory so its OpenCode config applies. This
   does not prompt agents. Session IDs are persisted by canonical project path
   under `${XDG_STATE_HOME:-~/.local/state}/kbots/projects/` with file mode
   `0600`; the script validates saved sessions before reuse and writes state
   atomically through a temporary file and rename.

The OpenCode service owns provider/model execution and saved provider
connections. The kbots client obtains service endpoint/auth headers through
the SDK and keeps auth headers internal. Never log or print credential values.

### Project bot configuration

Each registered project has a project-local `.agents/bot/` tree with the common
roles `ceo`, `git`, and `ops`. Role instructions, plugins, schedules, watcher
scripts, and resource limits belong to that project; setup must not copy them
from another checkout.

`.agents/bot/kbots.json` is the editable version 1 manifest and source of truth
for whether each role is enabled and its OpenCode settings. Its `opencode`
object can contain OpenCode config fields such as `instructions`, `plugins`,
`model`, and `permission`. `kbots project configure` writes
`.agents/bot/<role>/opencode.jsonc` for enabled roles, adds the OpenCode schema,
and resolves local plugin directory paths to absolute paths so service working
directories do not affect them. Existing generated files are preserved unless
`--overwrite` is supplied. Disabled roles are not generated.

`kbots project add` canonicalizes the existing project path, ensures role
folders and starter `AGENTS.md` files, and submits the registration request.
When no bot manifest or existing role OpenCode configs exist, it initializes
the manifest and configs. Existing role configs are preserved; if configs
exist without a manifest, settings must be migrated into the manifest before
regeneration. Registration upserts by canonical path in the daemon,
making repeated registration idempotent.

### Rust CLI

`cli/src/main.rs` defines the `kbots` command tree and routes commands into one
of three paths:

- **Local project configuration:** `project configure` invokes the Rust
  project-config module and does not need a daemon.
- **Direct OpenCode session start:** `sessions start` launches the Bun
  `start-sessions.ts` script and does not need a daemon.
- **Daemon RPC:** status, tasks, agents, project registration/list/show, and
  approvals are encoded as a single JSON-RPC request and sent through
  `kbots-ipc`. `project add` performs local scaffold preparation first, then
  sends the registration request. Responses are printed as JSON when stdout is
  not a TTY (or `--json` is passed); on a terminal, the CLI renders
  human-readable tables and summaries instead. Transport operations use
  10-second read/write timeouts and validate the JSON-RPC version and ID.
- **Daemon lifecycle / remote config (local):** `daemon start|stop|status`
  manages a detached process through a pid file; `connect --host`/`--local`
  saves or clears a remote endpoint. Neither path bypasses daemon RPC.

`kbots gui` and `kbots tui` launch the installed UI binaries. `KBOTS_GUI_BIN`
and `KBOTS_TUI_BIN` can override their paths. Endpoint selection is
`--host`/`KBOTS_HOST`, then `--socket`/`KBOTS_SOCKET`, then a saved remote host,
then `$XDG_RUNTIME_DIR/kbots.sock` → `$HOME/.local/run/kbots.sock` on Unix or
`127.0.0.1:7878` on Windows.

### GUI and TUI

The Slint GUI is an operator-facing daemon client. It issues calls for system
status, task list/create/show/cancel, project list/register/show, agents,
approvals, and recent events. It displays either returned JSON or connection
errors. It resolves socket configuration using the same override pattern as
the CLI. Its UI source is `gui/main.slint`; RPC integration is in
`gui/src/main.rs`.

The Ratatui TUI renders a live task-centric dashboard backed by the v1+v2
RPC surface: overview counts, real tasks/agents/approvals/activity, a task
composer (plain text creates a task; `@N text` routes to that task's CEO;
`/brief`, `/inbox`, `/done N`, `/blocked N`), and an inbox badge. It reconnects
on socket failure and polls only when the data may have changed. It is
intended for terminal environments including SSH, Mosh, tmux, or Herdr.
Exiting it must restore terminal state; it must not stop the daemon or its
agents. The TUI's narrow layout adapts below 112 columns.

Neither UI owns workflow policy, persistence, scheduling, or agent execution.
The daemon continues working if a UI disconnects or exits.

## Daemon boundary and IPC contract

The daemon is the single owner of mutable workflow state and side effects.
GUI, TUI, and CLI are clients. The transport contract in
`docs/kbots-ipc.md` is one newline-delimited JSON-RPC 2.0 request and response
per Unix socket or authenticated TCP connection. The CLI's endpoint lookup
order is documented above. The v1 RPC surface includes:

| Method | Purpose |
| --- | --- |
| `system.status` | Daemon and workflow summary |
| `tasks.create`, `tasks.list`, `tasks.get`, `tasks.cancel` | Task lifecycle; `tasks.create` auto-dispatches by default (`autoDispatch: false` opts out) |
| `tasks.message`, `tasks.messages`, `tasks.brief`, `tasks.inbox`, `tasks.update` | v2 task-centric dispatch: follow-up routing to the task's CEO, queued-message backlog, DB-composed briefings, unified needs-action inbox, operator status transitions |
| `router.run` | AI router: natural-language request → `create_task`/`continue_task`/`ask_user`; deterministic `@TASK-id` bypass, bounded yolo-mode Codex turn otherwise (`danger-full-access`, `approvalPolicy: never`; owner's 2026-11-19 directive); executes via `tasks.create`/`tasks.message` |
| `projects.register`, `projects.list`, `projects.get` | Project registry |
| `agents.list` | Bot roles, runs, session identifiers, and managed bots (`managed`) |
| `agents.start`, `agents.show`, `agents.stop`, `agents.interrupt`, `agents.prompt` | Managed bot lifecycle: exact session binding, hc-terminal mode only (headless retired 2026-11-19; legacy headless rows auto-close at reconcile), SQLite registry |
| `approvals.list`, `approvals.approve` | Protected operation approvals |
| `events.list` | Recent activity for the GUI |

Each request carries `jsonrpc: "2.0"`, an ID, method, and params; responses
carry the same ID and either a result or error. This is served by
`src/backend/daemon.ts` (`bun run daemon`). Creating a task records it
(status `queued` with v2) and, by default, auto-dispatches it to a per-task
CEO session (see "With v2" below). `tasks.cancel` is idempotent,
`projects.register` upserts by canonical path, `approvals.approve`
only records the decision (no protected operation is auto-executed), and
`agents.list` is read-only; the managed lifecycle (`agents.start/show/stop/
interrupt/prompt`) is documented in [`docs/kbots-ipc.md`](docs/kbots-ipc.md)
("Managed agents"): exact SDK session binding, compensation-based spawn with
a `starting → running → stopping → stopped/error` state model, boot-time
reconciliation, and prompts refused unless the agent is idle. Unknown
methods return `-32601`. Clients must not write workflow state directly.
With v2, `tasks.create` auto-dispatches to a per-task CEO session by default
(`autoDispatch: false` opts out); dispatch failure does not fail the RPC — the
result carries `dispatchError` and the task stays `queued`. `tasks.list`
accepts a `status` filter; `tasks.get` returns the `ceo` link and
`pendingMessageCount`. The v2 methods never round-trip to the model:
briefings and the inbox are composed from SQLite. The full v2 semantics,
schema v4 summary, and honest limitations are documented in
[`docs/kbots-ipc.md`](docs/kbots-ipc.md) and
[`docs/plans/2026-11-18-task-centric-mission-control.md`](docs/plans/2026-11-18-task-centric-mission-control.md).

## Planned workflow and persistence model

The intended backend separates responsibilities as follows:

1. The CLI or GUI submits a task or other operation over IPC.
2. The orchestrator validates the request and applies a code-defined workflow
   contract: stages, dependencies, acceptance criteria, retries, and required
   approvals.
3. Bot role policy selects the appropriate CEO planner/reviewer, serialized
   GIT or OPS worker, or coding worker. Deterministic permissions and gates
   remain in TypeScript; bots cannot grant themselves capabilities.
4. OpenCode and other adapters perform explicit external operations. The
   OpenCode adapter owns session lifecycle calls; Git, Herdr, and agmsg remain
   optional integrations behind their own adapters.
5. The database layer stores workflow state and an ordered event log as the
   source of truth. Transactions define durable state transitions.
6. On restart, the orchestrator reconciles persisted state with scheduled work
   and OpenCode sessions. It continues independently of connected clients.

This is the intended design. The v1 daemon implements minimal SQLite
persistence (`${XDG_STATE_HOME:-~/.local/state}/kbots/kbots.db`, WAL mode) and
the methods above; workflow execution, recovery, and the full orchestrator
remain unimplemented in the current tree.

## Configuration and local state

| Setting | Component | Meaning/default |
| --- | --- | --- |
| `OPENCODE_MODEL` | Smoke check | `provider/model`; defaults to `opencode/inkling-2-small-free` |
| `OPENCODE_DB_PATH` | Smoke check | Override OpenCode CLI database path |
| `KBOTS_CEO_CAP` | Daemon | Per-project active-CEO dispatch cap; defaults to 4 |
| `KBOTS_ORPHAN_REAP` | Daemon | Set to `off` to skip reclaiming unreferenced kbots terminals; default on |
| `KBOTS_SOCKET` | CLI, GUI, TUI | Overrides the default daemon Unix socket |
| `KBOTS_HOST`, `KBOTS_TCP` | CLI, GUI, TUI, daemon | TCP endpoint (`HOST:PORT`); `KBOTS_TCP=1` uses the default, and the daemon binds it |
| `KBOTS_TOKEN` | CLI, GUI, TUI | TCP auth token; defaults to `$HOME/.local/state/kbots/token` |
| `XDG_RUNTIME_DIR` | CLI, GUI, TUI | Supplies the default socket parent directory |
| `XDG_STATE_HOME` | Session startup; daemon | Supplies the root for saved session IDs and the daemon SQLite database |
| `KBOTS_INSTALL_DIR` | Installer | Defaults to `$HOME/.local/bin` (install.sh) or `%LOCALAPPDATA%\kbots\bin` (install.ps1) |
| `KBOTS_GUI_BIN`, `KBOTS_TUI_BIN` | CLI | Override the installed UI executable paths |

Session IDs are local state, not credentials. Provider secrets stay managed by
OpenCode. Project instructions, plugin configuration, and role policy remain
inside each project checkout.

## Build and change guide

The repository has two build ecosystems: Bun/TypeScript for the backend and
Cargo for the Rust clients plus the shared `ipc/` crate. On Unix hosts,
`./install.sh` installs frozen Bun dependencies, runs `bun run tsc --noEmit`,
builds the GUI, TUI, and CLI in release mode, installs their binaries, writes
the desktop launcher, and installs and restarts the `kbots-daemon` systemd user
service from `deploy/kbots-daemon.service` (opt out with
`KBOTS_SKIP_DAEMON_UNIT=1`). On Windows,
`powershell -ExecutionPolicy Bypass -File .\install.ps1` performs the same
builds per-user, updates the user PATH, and starts a detached TCP daemon with
no service or admin rights. On Nix-managed hosts,
`kbdeploy "what changed"` is the one deploy command;
`nix build .#` is the packaging check and `nix develop` provides the toolchain.
After every application code change, clients and the daemon must always run
freshly deployed code. See `docs/deployment.md`.

For detailed contracts, see:

- [`docs/kbots-ipc.md`](docs/kbots-ipc.md) for socket framing, methods, and
  project registration/configuration behavior.
- [`docs/opencode-control.md`](docs/opencode-control.md) for OpenCode SDK,
  provider, and session operations.
- [`cli/README.md`](cli/README.md), [`gui/README.md`](gui/README.md), and
  [`tui/README.md`](tui/README.md) for client-specific behavior.
- The `README.md` files under `src/backend/` for planned module ownership.
