# Automation CLI

`kbots` is a Rust command-line client for automation. It exposes stable
subcommands and JSON output, then sends daemon-owned operations over a local
Unix domain socket. It does not implement orchestration or bypass daemon-side
authorization and idempotency checks.

Examples:

```sh
kbots status
kbots tasks add "Implement authentication" --project api-service
kbots tasks list --project api-service
kbots tasks show TASK-104
kbots agents list
kbots project add /path/to/project
kbots project add --name "My Project"   # current directory
kbots project list
kbots project show PROJECT-ID
kbots approve DEPLOY-23
kbots tui
kbots gui
```

Use `--socket PATH` or `KBOTS_SOCKET` to select the daemon socket. By default,
the CLI uses `$XDG_RUNTIME_DIR/kbots.sock`, falling back to
`$HOME/.local/run/kbots.sock`. The GUI and TUI paths can be overridden with
`KBOTS_GUI_BIN` and `KBOTS_TUI_BIN`.

The Bun daemon and JSON-RPC server are not implemented yet. Until they are,
`status`, `tasks`, `projects`, `agents`, and `approve` return a clear connection
error. `project add` still prepares the local bot-role folder scaffold before
the daemon request. Its directory structure matches the `ceo/`, `git/`, and
`ops/` roles used in `kt-system` and `nsac`; project-specific instructions and
OpenCode config are never copied between repositories.
Their planned methods and newline-delimited JSON-RPC contract are documented
in [`../docs/kbots-ipc.md`](../docs/kbots-ipc.md).
