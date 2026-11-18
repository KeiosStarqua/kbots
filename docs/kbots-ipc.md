# kbots local IPC contract

The Bun daemon owns workflow state and side effects. GUI, TUI, and CLI are
clients. The initial transport contract is one newline-delimited JSON-RPC 2.0
request and response per Unix domain socket connection. The daemon is
implemented (v1 minimal) and runs as the `kbots-daemon` systemd user service
after `./install.sh` (opt out with `KBOTS_SKIP_DAEMON_UNIT=1`), or manually
with `bun run daemon` (equivalently `bun run src/backend/daemon.ts`) without
systemd.

The CLI discovers the socket in this order:

1. `--socket PATH`
2. `KBOTS_SOCKET`
3. `$XDG_RUNTIME_DIR/kbots.sock`
4. `$HOME/.local/run/kbots.sock`

Example request:

```json
{"jsonrpc":"2.0","id":1,"method":"tasks.create","params":{"title":"Implement authentication","project":"api-service"}}
```

Example response:

```json
{"jsonrpc":"2.0","id":1,"result":{"taskId":"TASK-104","status":"planning"}}
```

Project registration request:

```json
{"jsonrpc":"2.0","id":1,"method":"projects.register","params":{"path":"/home/keios/repos/mein/kt-system","name":"kt-system","botRoles":["ceo","git","ops"]}}
```

Initial CLI methods:

| CLI command | RPC method | Parameters |
| --- | --- | --- |
| `status` | `system.status` | `{}` |
| `tasks add TITLE [--project ID]` | `tasks.create` | `title`, optional `project` |
| `tasks list [--project ID]` | `tasks.list` | optional `project` |
| `tasks show ID` | `tasks.get` | `taskId` |
| `tasks cancel ID` | `tasks.cancel` | `taskId` |
| `agents list` | `agents.list` | `{}` |
| `project add [PATH] [--name NAME]` | `projects.register` | canonical `path`, display `name`, bot roles |
| `project configure [PATH] [--overwrite]` | local CLI action | generate per-bot OpenCode configs from the manifest |
| `sessions start [--directory PATH]` | local OpenCode client action | create or reuse idle sessions for enabled roles |
| `project list` | `projects.list` | `{}` |
| `project show ID` | `projects.get` | `projectId` |
| `approve ID` | `approvals.approve` | `approvalId` |

Result shapes for the read and create methods:

| Method | Result |
| --- | --- |
| `tasks.create` | `{taskId, status}` (`status` is `"planning"`) |
| `tasks.list` | `{items: [...]}` |
| `tasks.get` | task object with `taskId` |
| `tasks.cancel` | `{taskId, status}` |
| `approvals.list` | `{items: [...]}` |
| `events.list` | `{items: [...]}` (newest first; `limit` defaults to 100, max 500) |
| `agents.list` | `{items: [{projectId?, projectPath, role, enabled, sessionID\|null, directory, registered}]}` |

The desktop GUI also requests `approvals.list` and `events.list` (with an
optional `limit`) to populate the approvals and activity views. These read
methods return an `items` array in `result`, as the GUI expects. The daemon is
implemented (v1), so GUI operations complete when it is running; the Ratatui
TUI is still demo-only and does not connect.

For `projects.register`, the CLI first canonicalizes the existing directory,
creates missing `.agents/bot/{ceo,git,ops}` folders and starter `AGENTS.md`
files, and generates the initial manifest/configs only when no per-bot
`opencode.jsonc` already exists. It does not overwrite existing files or copy
bot configs from another project. The daemon upserts registrations by
canonical path so repeated adds are idempotent.

`kbots project configure [PATH]` is local and does not need the daemon. The
editable `.agents/bot/kbots.json` has `version: 1` and a `bots` object with
`ceo`, `git`, and `ops` entries. Each entry has `enabled` and an `opencode`
object containing OpenCode config fields such as `instructions`, `plugins`,
`model`, and `permission`. It writes `.agents/bot/<role>/opencode.jsonc` for
enabled roles and preserves existing outputs by default. Use `--overwrite` to
regenerate from the manifest. Local plugin paths are made absolute so they
resolve consistently when OpenCode runs from a service working directory.
When a project already has OpenCode configs but no manifest, create the
manifest from those existing settings before using `--overwrite`.

`kbots sessions start` reads the enabled roles from the manifest and creates
one idle session in each role directory. It records IDs in the user's local
state directory and checks saved IDs against OpenCode before reusing them.
Session creation does not prompt the agents. This local action talks directly
to the OpenCode service and remains usable without the workflow daemon.

## Bot folder convention

The `kt-system` and `nsac` references share the `ceo/`, `git/`, and `ops/` bot
roles. Their role instructions, OpenCode configs, plugins, watcher scripts,
schedules, and resource limits are project-specific. Registration guarantees
the common directories only; it leaves those project contracts to each repo.

## Daemon status (v1)

The daemon exists and serves the methods above. It stores state in SQLite at
`${XDG_STATE_HOME:-~/.local/state}/kbots/kbots.db` (WAL mode). It creates the
socket's parent directory, unlinks stale sockets, sets the socket mode to
`0600`, and cleans up the socket on SIGINT/SIGTERM. Unknown methods return
JSON-RPC `-32601`.

Honest v1 semantics:

- `tasks.create` records the task (status `"planning"`) and logs an event.
  Nothing auto-dispatches tasks to a bot yet; the orchestrator/workflow engine
  is still planned.
- `tasks.cancel` is idempotent.
- `projects.register` upserts by canonical path (idempotent).
- `approvals.approve` records the approve decision; no protected operation is
  auto-executed.
- `agents.list` merges the project registry, each project's
  `.agents/bot/kbots.json` manifest, and saved session IDs from
  `${XDG_STATE_HOME:-~/.local/state}/kbots/projects/*.json`. It is read-only
  and does not prompt or start agents.
- `events.list` returns the ordered event log, newest first, with a `limit`
  parameter (default 100, max 500).
- No OpenCode prompting is wired to tasks yet. The daemon does not embed
  provider credentials, session inspection stays read-only, and clients never
  write workflow state directly.

Mutating methods are idempotent where noted above; broader permission, lock,
and approval enforcement for mutating methods is still incomplete.
