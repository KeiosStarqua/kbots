# kbots Architecture

## Purpose and status

`kbots` is a local coding workflow orchestrator. Its intended runtime has one
long-lived Bun daemon that owns project/workflow state and OpenCode sessions,
with Rust command-line, desktop, and terminal clients. The repository currently
contains the OpenCode smoke/control utilities, the minimal v1 workflow daemon,
project configuration and idle session setup, and the Rust clients. The
daemon is implemented in minimal v1 form (`src/backend/daemon.ts`, started
with `bun run daemon`, deployed as the `kbots-daemon` systemd user service by
`./install.sh`); the orchestrator/workflow engine, scheduling, and full
persistence repositories remain planned. The IPC contract in
`docs/kbots-ipc.md` is served by the v1 daemon. The terminal
UI is demo-only. The Slint GUI connects to the v1 daemon.

This distinction matters when reading the design below: **implemented** means
the code exists in this repository; **planned** means the responsibility or
contract is documented but still needs its owning backend implementation.

## System shape

```text
                    ┌───────────────────────────────┐
                    │ Local project directories     │
                    │ .agents/bot/{ceo,git,ops}      │
                    │ kbots.json + OpenCode configs  │
                    └──────────────┬────────────────┘
                                   │ project setup / session start
                                   ▼
┌────────────────┐      Unix socket      ┌──────────────────────────────┐
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

The socket path is the local interface served by the v1 daemon. Until it is
running, daemon-oriented CLI calls fail with a socket connection error.
`project configure` and `sessions start` are local/direct OpenCode operations
and work without that daemon.

## Repository ownership

| Path | Responsibility | State |
| --- | --- | --- |
| `src/backend/cli/` | Transitional Bun command entry points for OpenCode control and bot session startup | Implemented |
| `src/backend/opencode/` | Shared OpenCode service/session adapter | Planned module boundary; behavior currently lives in CLI scripts |
| `src/backend/orchestrator/` | v1 task/project/approval/system services; file-change watcher (`orchestrator/watchers.ts`) prompts the managed git agent on working-tree deltas with debounce + cooldown; scheduling, workflow state transitions, event ingestion, restart reconciliation | v1 services + git watcher implemented; workflow engine and scheduling still planned |
| `src/backend/workflows/` | Task stages, dependencies, acceptance criteria, retry and approval rules | Planned |
| `src/backend/bots/` | agents.list implemented (incl. managed `agents.start/show/stop/prompt/interrupt` lifecycle: exact session binding, terminal+headless modes, SQLite registry, boot reconciliation); CEO planning/review, GIT and OPS work contracts, coding worker policies | Lifecycle implemented; work contracts still planned |
| `src/backend/adapters/` | Herdr adapter (read-only `hc ls`) implemented; Git and optional agmsg integrations | Herdr implemented; Git/agmsg planned |
| `src/backend/db/` | SQLite schema, migrations, repositories, transaction boundaries | Implemented minimal persistence (WAL) |
| `src/backend/daemon.ts` | Bun daemon entry point: newline-delimited JSON-RPC over a Unix domain socket, SQLite-backed | Implemented (v1 minimal); deployed as the `kbots-daemon` systemd user service by install.sh (opt out with `KBOTS_SKIP_DAEMON_UNIT=1`) |
| `cli/` | Rust `kbots` automation client, local project scaffolding/configuration, UI launch | Implemented |
| `gui/` | Native Slint operator client | UI and RPC calls implemented; connects to the v1 daemon |
| `tui/control-center/` | Native Ratatui dashboard | Demo scaffold; no daemon connection |
| `docs/` | Detailed OpenCode and IPC operational contracts | Implemented documentation |
| `install.sh` | Frozen Bun install, TypeScript check, Rust builds, binary and desktop launcher install | Implemented |

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
   default to `KIR_AUTO_DIRECTORY` or the sibling
   `film-production/.agents/bot/kir-auto` directory; other control commands
   default to the current directory. Prompt waits are capped at two minutes.
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
  approvals are encoded as a single JSON-RPC request and sent to the Unix
  socket. `project add` performs local scaffold preparation first, then sends
  the registration request. Responses are printed as JSON when stdout is not a
  TTY (or `--json` is passed); on a terminal, the CLI renders human-readable
  tables and summaries instead. Socket operations
  use 15-second read/write timeouts and validate the JSON-RPC version and ID.

`kbots gui` and `kbots tui` launch the installed UI binaries. `KBOTS_GUI_BIN`
and `KBOTS_TUI_BIN` can override their paths. Global socket selection is
`--socket PATH`, then `KBOTS_SOCKET`, then `$XDG_RUNTIME_DIR/kbots.sock`, then
`$HOME/.local/run/kbots.sock`.

### GUI and TUI

The Slint GUI is an operator-facing daemon client. It issues calls for system
status, task list/create/show/cancel, project list/register/show, agents,
approvals, and recent events. It displays either returned JSON or connection
errors. It resolves socket configuration using the same override pattern as
the CLI. Its UI source is `gui/main.slint`; RPC integration is in
`gui/src/main.rs`.

The Ratatui TUI currently renders demo data and navigation for overview,
projects, tasks, agents, approvals, and activity. It does not connect to the
socket yet. It is intended for terminal environments including SSH, Mosh,
tmux, or Herdr. Exiting it must restore terminal state; it must not stop the
daemon or its agents. The TUI's narrow layout adapts below 112 columns.

Neither UI owns workflow policy, persistence, scheduling, or agent execution.
The daemon continues working if a UI disconnects or exits.

## Daemon boundary and IPC contract

The daemon is the single owner of mutable workflow state and side effects.
GUI, TUI, and CLI are clients. The transport contract in
`docs/kbots-ipc.md` is one newline-delimited JSON-RPC 2.0 request and response
per Unix domain socket connection. The CLI's socket lookup order is documented
above. The v1 RPC surface includes:

| Method | Purpose |
| --- | --- |
| `system.status` | Daemon and workflow summary |
| `tasks.create`, `tasks.list`, `tasks.get`, `tasks.cancel` | Task lifecycle |
| `projects.register`, `projects.list`, `projects.get` | Project registry |
| `agents.list` | Bot roles, runs, session identifiers, and managed bots (`managed`) |
| `agents.start`, `agents.show`, `agents.stop`, `agents.interrupt`, `agents.prompt` | Managed bot lifecycle: exact session binding, terminal+headless modes, SQLite registry |
| `approvals.list`, `approvals.approve` | Protected operation approvals |
| `events.list` | Recent activity for the GUI |

Each request carries `jsonrpc: "2.0"`, an ID, method, and params; responses
carry the same ID and either a result or error. This is a v1 contract served by
`src/backend/daemon.ts` (`bun run daemon`). Creating a task records it
(status `"planning"`) and logs an event; nothing auto-dispatches it to a bot
yet, and tasks are not wired to OpenCode prompting. `tasks.cancel` is
idempotent, `projects.register` upserts by canonical path, `approvals.approve`
only records the decision (no protected operation is auto-executed), and
`agents.list` is read-only; the managed lifecycle (`agents.start/show/stop/
interrupt/prompt`) is documented in [`docs/kbots-ipc.md`](docs/kbots-ipc.md)
("Managed agents"): exact SDK session binding, compensation-based spawn with
a `starting → running → stopping → stopped/error` state model, boot-time
reconciliation, and prompts refused unless the agent is idle. Unknown
methods return `-32601`. Clients must not write workflow state directly.

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
| `KIR_AUTO_DIRECTORY` | Session control | Overrides sibling `film-production/.agents/bot/kir-auto` default |
| `KBOTS_SOCKET` | CLI and GUI | Overrides the default daemon socket |
| `XDG_RUNTIME_DIR` | CLI and GUI | Supplies the default socket parent directory |
| `XDG_STATE_HOME` | Session startup; daemon | Supplies the root for saved session IDs and the daemon SQLite database |
| `KBOTS_INSTALL_DIR` | Installer | Defaults to `$HOME/.local/bin` |
| `KBOTS_GUI_BIN`, `KBOTS_TUI_BIN` | CLI | Override the installed UI executable paths |

Session IDs are local state, not credentials. Provider secrets stay managed by
OpenCode. Project instructions, plugin configuration, and role policy remain
inside each project checkout.

## Build and change guide

The repository has two build ecosystems: Bun/TypeScript for the backend and
Cargo for the Rust clients. `./install.sh` installs frozen Bun dependencies,
runs `bun run tsc --noEmit`, builds the GUI, TUI, and CLI in release mode,
installs their binaries, writes the desktop launcher, and installs and
restarts the `kbots-daemon` systemd user service from
`deploy/kbots-daemon.service` (opt out with `KBOTS_SKIP_DAEMON_UNIT=1`). After
every application code change, `./install.sh` is the deploy path — clients and
the daemon must always run freshly deployed code.

For detailed contracts, see:

- [`docs/kbots-ipc.md`](docs/kbots-ipc.md) for socket framing, methods, and
  project registration/configuration behavior.
- [`docs/opencode-control.md`](docs/opencode-control.md) for OpenCode SDK,
  provider, and session operations.
- [`cli/README.md`](cli/README.md), [`gui/README.md`](gui/README.md), and
  [`tui/README.md`](tui/README.md) for client-specific behavior.
- The `README.md` files under `src/backend/` for planned module ownership.
