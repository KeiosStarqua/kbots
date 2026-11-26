# kbots

`kbots` is a local coding workflow orchestrator workspace. Its Bun/TypeScript
backend manages OpenCode sessions and workflow automation; native Slint and
Ratatui apps provide desktop and terminal operator interfaces.

The package has three entry points:

- **`smoke`** runs an embedded OpenCode host in the current process, checks
  provider access, sends one test prompt, prints the response, and removes its
  temporary session.
- **`control`** connects to the registered local OpenCode service. It can
  report service and provider information, inspect sessions, send prompts, or
  interrupt a session's current run.
- **`daemon`** starts the Bun daemon: newline-delimited JSON-RPC
  2.0 over a Unix domain socket, backed by SQLite. Run it with
  `bun run daemon`. It serves the v1 surface plus the v2
  task-centric dispatch surface documented in
  [`docs/kbots-ipc.md`](docs/kbots-ipc.md) (auto-dispatch on task creation,
  per-task CEO sessions, briefings, inbox).

## Control Center starter

The workspace separates the Bun backend in `src/backend/` from the Rust/Slint
desktop client in `gui/` and the Rust/Ratatui terminal client in
`tui/control-center/`. Both UI clients connect to one Bun daemon. The dashboards
include navigation, project/task/agent/approval summaries, agent activity, a task
queue, and recent events. The TUI renders live data (plus a task composer with
`/brief`, `/inbox`, `/done`, `/blocked`) and requires the daemon; the
`kbots` automation entry
point is in `cli/` and uses the v1 JSON-RPC socket contract.

## Install

### Windows (native, no WSL or admin rights)

Install [Bun](https://bun.sh) and Rust via Scoop (`scoop install bun rustup
mingw`), then from this checkout run:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

It type-checks the backend, builds all three Rust clients, installs `kbots.exe`,
`kbots-tui.exe`, `kbots-gui.exe` and a `kb` shim into
`%LOCALAPPDATA%\kbots\bin`, adds that folder to the per-user PATH, records the
repo root for the background daemon, adds a Start Menu shortcut, and starts the
daemon. `-NoDaemon`, `-NoPath`, `-Autostart` and `-InstallDir PATH` adjust those
steps. Nothing writes to system directories or needs elevation.

Alternatively, install with Scoop from the personal bucket:

```powershell
scoop bucket add kbots https://github.com/KeiosStarqua/kbots
scoop install kbots
```

The manifest builds from a pinned `deploy/YYYY.MM.DD.n` source tag, so `bun`,
`rustup`, `mingw` and `git` must be installed (all are in Scoop's `main` bucket).
It installs the `kbots`, `kb`, `kbots-tui` and `kbots-gui` shims and a Start Menu
shortcut, but intentionally does not start the daemon — run
`kbots daemon start` first. This is the same package `skoopub` installs through
`scoop import scoop.json`.

Windows clients and the daemon speak authenticated TCP on `127.0.0.1:7878`
(the port is configurable with `--tcp HOST:PORT` on the daemon and `--host` on
clients). To control an existing Linux daemon over Tailscale instead of running
one locally:

```powershell
kbots connect --host 100.x.x.x:7878   # set KBOTS_TOKEN if the daemon's token file is not readable
kb status
kb tui --full
kbots connect --local                 # back to the local daemon
```

The TUI's embedded Codex terminal is owned by Herdr Core; on Windows set
`HC_SOCKET=tcp://HOST:PORT` if `hc` is reachable over TCP, otherwise use
`kb tui --full` for the dashboard without the embedded terminal.

### Unix (Nix or the shell installer)

With Nix (flake + Home Manager), all clients and the daemon come from the
store:

```sh
nix build .#                 # kbots, kb, kbots-tui, kbots-gui, kbots-daemon, kbdeploy
nix develop                  # Bun + Rust + Slint build deps

# In your home-manager flake:
# inputs.kbots = { url = "git+file:///path/to/kbots"; inputs.nixpkgs.follows = "nixpkgs"; };
# modules = [ ./home.nix inputs.kbots.homeModules.default ];
services.kbots.enable = true;
```

After that, every code change is one command:

```sh
kbdeploy "what changed"   # commit + deploy tag + push + flake update + HM switch + kb status
```

The module installs the package into `home.packages` and runs
`kbots-daemon.service` from the package, with PATH pointing at the external
`hc`/`opencode`/`codex` programs. See
[`docs/deployment.md`](docs/deployment.md#nix-flake-the-install-path-on-nix-managed-hosts).

On a non-Nix host, install the latest GUI, TUI, and CLI binaries and verify
the TypeScript backend with:

```sh
./install.sh
```

The installer adds a desktop launcher at
`${XDG_DATA_HOME:-$HOME/.local/share}/applications/kbots.desktop` and uses
`~/.local/bin` for binaries by default. Set `KBOTS_INSTALL_DIR` to use a
different destination, for example `KBOTS_INSTALL_DIR="$HOME/bin" ./install.sh`.
It does not use `sudo`. `install.sh` also installs `kb`, a short alias for
`kbots`. The installer also installs and restarts the
`kbots-daemon` systemd user service (from `deploy/kbots-daemon.service`) so the
daemon is always running freshly deployed code; opt out with
`KBOTS_SKIP_DAEMON_UNIT=1`. Without a user systemd session, start the daemon
manually with `bun run daemon`.

Run the desktop starter with:

```sh
cargo run --manifest-path gui/Cargo.toml
```

Start the terminal dashboard in Herdr, tmux, SSH, or Mosh with:

```sh
cargo run --manifest-path tui/control-center/Cargo.toml
```

By default the TUI is a minimal Codex chat terminal: one fullscreen embedded
Codex CLI (`codex --dangerously-bypass-approvals-and-sandbox`, owned by herdr-core)
plus a one-line header (`kbots · CODEX · LIVE`) and a `✕ exit` button (or
`Ctrl+]`) to quit. `kb tui --full` opens the complete operator dashboard
(Overview, Projects, Tasks, Agents, Approvals, Activity log). The CLI can
launch either UI and defines automation commands for status, tasks, projects,
agents, and approvals. Until the daemon is running, those API commands return
a clear socket connection error.

Register each local project (or run `kbots project add` from inside its root):

```sh
kbots project add /home/keios/repos/mein/kt-system
kbots project add /home/keios/repos/mein/nsac --name NSAC
kbots project list
kbots project show <project-id>
kbots sessions start --directory /home/keios/repos/mein/nsac
```

Registration canonicalizes the project path and creates missing bot-role
folders and starter instruction files. For a new bot setup, it also writes an
editable `.agents/bot/kbots.json` manifest and generates each role's
`opencode.jsonc`. Existing project configs are preserved. Edit the manifest
and run `kbots project configure --overwrite` to regenerate them. Registry
persistence is handled by the Bun daemon's local SQLite database. The daemon
runs as the `kbots-daemon` systemd user service from `./install.sh` on
non-Nix hosts, or from the Nix Home Manager module (`services.kbots.enable`).

Start the idle OpenCode sessions for enabled roles with `kbots sessions start`.
The command reads `.agents/bot/kbots.json`, uses each bot directory so its
OpenCode config applies, and stores reusable session IDs in the user's
`~/.local/state/kbots/projects/` directory. It does not send prompts; the
sessions wait for work. It talks directly to the local OpenCode service, so
the workflow daemon is not needed for this command.

See [`gui/README.md`](gui/README.md) and [`tui/README.md`](tui/README.md) for
the clients and planned IPC boundary, and
[`src/backend/README.md`](src/backend/README.md) for backend module ownership.
See [`cli/README.md`](cli/README.md) for the CLI commands and
[`docs/kbots-ipc.md`](docs/kbots-ipc.md) for their JSON-RPC contract,
[`docs/plans/2026-11-18-task-centric-mission-control.md`](docs/plans/2026-11-18-task-centric-mission-control.md)
for the task-centric Mission Control design.

## Requirements and setup

- [Bun](https://bun.sh/) installed and available as `bun`.
- OpenCode installed locally, with a saved provider connection for a model you
  can use.
- For `control`, the laptop's registered OpenCode service. `Service.ensure()`
  reuses it if running and starts it otherwise.

Install the exact dependency versions pinned by this package:

```sh
bun install
```

The package uses `@opencode/sdk`, `@opencode/client`, and `@opencode/plugin`
version `2.3.2`. TypeScript is pinned as a development dependency.

## Embedded SDK smoke check

Run the smoke check with:

```sh
bun run smoke
```

The check uses `@opencode/sdk` to start an embedded host, registers a small
plugin, reads provider metadata and the active OpenCode account connection,
creates a session, asks for the exact response `OPENCODE_GO_SDK_OK`, waits up
to two minutes, and prints the assistant response. It then removes the
temporary session, including when the prompt fails.

By default, the selected model is
`opencode/inkling-2-small-free`. Select another available model using
`provider/model` format:

```sh
OPENCODE_MODEL=opencode-go/deepseek-v4.1-flash bun run smoke
```

The embedded host uses the database path reported by
`opencode debug paths db`, allowing it to read the same saved provider
connection as the OpenCode CLI. Set `OPENCODE_DB_PATH` to override that path.
The script does not print database contents or credentials.

## Local service control

Run `control` without arguments to display the registered service's version,
process ID, endpoint, and providers available from the current directory:

```sh
bun run control
bun run control --directory /absolute/project/path
```

It does not create a session or send a prompt unless `--prompt` is supplied.
To create a session, send a prompt, and keep the session for later use:

```sh
bun run control \
  --prompt "Summarize the current project" \
  --model opencode-go/deepseek-v4.1-flash \
  --directory /absolute/project/path
```

`--model` is optional and must use `provider/model` format. If omitted, the
service chooses its default model. `--directory` chooses the project context;
for a new-session prompt it defaults to the current working directory. The
command prints the new session ID and assistant response. It waits up to two
minutes for a response.

### Inspect and manage sessions

Session listing and existing-session commands use the current directory by
default. Select another directory for one invocation with `--directory`.

```sh
# Show sessions in the current directory
bun run control --list-sessions

# Inspect one session without changing it
bun run control --session <session-id>

# Continue the conversation in an existing session
bun run control --session <session-id> --prompt "Report the current step and wait"

# Stop the session's current run
bun run control --session <session-id> --interrupt
```

Existing-session actions verify that the selected session belongs to the
selected directory. Listing and inspection are read-only. Prompting adds to
the existing conversation. Interrupting stops only the current run; it does
not delete the session or disable any recurring schedule attached to it. A
schedule may start another run later.

### Command options

```text
--help, -h                 Show usage
--list-sessions            List sessions in the selected directory
--session <id>             Select an existing session
--prompt <text>             Prompt a new or selected session
--interrupt                 Interrupt the selected session's current run
--model <provider/model>    Choose a model when creating a session
--directory <path>          Select the project/session directory
```

`--list-sessions` cannot be combined with session actions. `--interrupt`
requires `--session` and cannot be combined with `--prompt`.

## AI router (natural language routing)

The composer accepts plain natural language: the client sends it to the
daemon's `router.run` method, which decides where the request goes. The
router is a thin proposal layer — it never calls agents directly. Confident
`create_task` / `continue_task` decisions execute through the same paths as
`tasks.create` and `tasks.message`, so dispatch caps, queued-message
semantics and approvals apply unchanged. An explicit `@TASK-id` reference
(or a selected task) routes deterministically without any AI call. When the
router is unsure it returns `ask_user` with a short question and candidate
tasks instead of acting; nothing is created, messaged, or dispatched in that
case. The CLI is unchanged; this is local inference, not a guarantee of
model quality — treat routing as a convenience, not an authority.

## Configuration and data handling

| Setting | Used by | Default |
| --- | --- | --- |
| `OPENCODE_MODEL` | `smoke` | `opencode/inkling-2-small-free` |
| `OPENCODE_DB_PATH` | `smoke` | Path from `opencode debug paths db` |
| `KBOTS_CEO_CAP` | Daemon | Per-project active-CEO dispatch cap; defaults to 4 |
| `KBOTS_ROUTER_MIN_CONFIDENCE` | Daemon | Minimum router confidence for auto-execution; defaults to 0.65 |
| `KBOTS_SOCKET` | CLI, GUI, TUI | Overrides the default daemon socket |
| `--model` | `control` new sessions | Service default |
| `--directory` | `control` | Current directory |

The client obtains the service endpoint and authentication header through
`@opencode/client`. The header is applied internally and is never printed.
Provider credentials are read through OpenCode; `kbots` does not print
credential values or auth headers.

Prompts and interrupts are explicit actions. The control tool does not delete
sessions. The smoke check is the exception: it removes only the temporary
session it created.

## Development and reference

Run the TypeScript check with:

```sh
bun run tsc --noEmit
```

For the embedded host setup, provider behavior, service details, and session
control notes, see [`docs/opencode-control.md`](docs/opencode-control.md).
