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
kbots project configure /path/to/project
kbots project configure --overwrite      # regenerate current project's OpenCode configs
kbots sessions start --directory /path/to/project
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

The Bun daemon and JSON-RPC server exist (v1 minimal). Start it with
`bun run daemon` from the repo (or `bun run src/backend/daemon.ts`). Until it
is running, `status`, `tasks`, `projects`, `agents`, and `approve` return a
clear connection error. `project add` prepares missing
`.agents/bot/{ceo,git,ops}` folders and starter `AGENTS.md` files before the
daemon request. If the project has no
existing per-bot `opencode.jsonc`, it also creates `.agents/bot/kbots.json` and
renders one OpenCode config per enabled role. The manifest is the editable
source of truth; put OpenCode config keys under that role's `opencode` object
(for example `instructions`, `plugins`, `model`, or `permission`), then run
`kbots project configure` to generate missing configs. Existing configs are
preserved unless `--overwrite` is supplied. Local plugin paths in the manifest
are resolved relative to the project and written as absolute paths so service
working directories do not change their meaning. Generated JSONC files use
valid JSON syntax. The next `--overwrite` replaces them from the manifest.

Projects that already have per-bot OpenCode configs are not auto-migrated:
copy their settings into `.agents/bot/kbots.json`, then run
`kbots project configure --overwrite`. Project-specific instructions and
plugins stay local instead of being copied between repositories.

`kbots sessions start` creates one idle OpenCode session for every enabled bot
in the manifest, using each role's `.agents/bot/<role>` directory so its
`opencode.jsonc` applies. It does not send a prompt or start agent work. Session
IDs are stored with mode `0600` under `${XDG_STATE_HOME:-~/.local/state}/kbots/projects/`
and reused on later runs when the OpenCode service still has them. The command
connects to or starts the local OpenCode service; it does not require the
workflow daemon.
The methods and newline-delimited JSON-RPC contract are documented
in [`../docs/kbots-ipc.md`](../docs/kbots-ipc.md).
