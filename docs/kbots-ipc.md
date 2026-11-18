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
| `project list` | `projects.list` | `{}` |
| `project show ID` | `projects.get` | `projectId` |
| `approve ID` | `approvals.approve` | `approvalId` |

For `projects.register`, the CLI first canonicalizes the existing directory and
ensures `.agents/bot/ceo`, `.agents/bot/git`, and `.agents/bot/ops` exist. It
does not overwrite existing files or copy bot configs from another project.
The daemon should upsert registrations by canonical path so repeated adds are
idempotent.

## Bot folder convention

The `kt-system` and `nsac` references share the `ceo/`, `git/`, and `ops/` bot
roles. Their role instructions, OpenCode configs, plugins, watcher scripts,
schedules, and resource limits are project-specific. Registration guarantees
the common directories only; it leaves those project contracts to each repo.

This is a proposed v1 contract, not a running API. The daemon must validate
permissions, locks, approvals, and idempotency for every mutating method.
