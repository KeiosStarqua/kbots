# kbots local IPC contract

The Bun daemon owns workflow state and side effects. GUI, TUI, and CLI are
clients. The initial transport contract is one newline-delimited JSON-RPC 2.0
request and response per Unix domain socket connection.

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

The desktop GUI also requests `approvals.list` and `events.list` (with an
optional `limit`) to populate the approvals and activity views. These read
methods should return an `items` array in `result`. The daemon executable is
not implemented yet, so GUI operations report a connection or method error
until a compatible daemon is running.

For `projects.register`, the CLI first canonicalizes the existing directory,
creates missing `.agents/bot/{ceo,git,ops}` folders and starter `AGENTS.md`
files, and generates the initial manifest/configs only when no per-bot
`opencode.jsonc` already exists. It does not overwrite existing files or copy
bot configs from another project. The daemon should upsert registrations by
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
to the OpenCode service and remains available before the workflow daemon is
implemented.

## Bot folder convention

The `kt-system` and `nsac` references share the `ceo/`, `git/`, and `ops/` bot
roles. Their role instructions, OpenCode configs, plugins, watcher scripts,
schedules, and resource limits are project-specific. Registration guarantees
the common directories only; it leaves those project contracts to each repo.

This is a proposed v1 contract, not a running API. The daemon must validate
permissions, locks, approvals, and idempotency for every mutating method.
