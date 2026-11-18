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

Initial CLI methods:

| CLI command | RPC method | Parameters |
| --- | --- | --- |
| `status` | `system.status` | `{}` |
| `tasks add TITLE [--project ID]` | `tasks.create` | `title`, optional `project` |
| `tasks list [--project ID]` | `tasks.list` | optional `project` |
| `tasks show ID` | `tasks.get` | `taskId` |
| `tasks cancel ID` | `tasks.cancel` | `taskId` |
| `agents list` | `agents.list` | `{}` |
| `approve ID` | `approvals.approve` | `approvalId` |

This is a proposed v1 contract, not a running API. The daemon must validate
permissions, locks, approvals, and idempotency for every mutating method.
