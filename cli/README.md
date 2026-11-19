# Automation CLI

`kbots` is a Rust command-line client for automation. `kb` is a short
alias `install.sh` links to it (`~/.local/bin/kb` → `kbots`). It exposes stable
subcommands, then sends daemon-owned operations over a local Unix domain
socket. It does not implement orchestration or bypass daemon-side
authorization and idempotency checks. When stdout is a terminal, output is a
human-readable rendering (tables and summaries); when stdout is piped — or
`--json` is passed — output is pretty-printed JSON, so scripts keep a stable
machine-readable contract.

Examples:

```sh
kbots status
kbots tasks add "Implement authentication" --project api-service --goal "Ship JWT login" --detail "See docs/auth.md for the rollout plan"
kbots tasks list --project api-service
kbots tasks show TASK-104
kbots tasks message TASK-104 "Please also update the tests"
kbots tasks messages TASK-104
kbots tasks brief
kbots tasks brief TASK-104
kbots tasks inbox
kbots tasks update TASK-104 --status review --note "Ready for review"
kbots agents list
kbots agents list --project PRJ-1   # scope to one project when you have many
kbots ag ls -p PRJ-1               # same, short form (alias: ag, ls, -p)
# Inside a folder that has a `.agents/bot/` marker (a project kbots manages),
# `ag ls` and `tasks ls` default to the current project — no --project needed.
# Passing --project always wins; outside any project the unfiltered list is shown.
kbots prj ls          # `prj` for `project`, `ls` for `list`
kbots prj watch --project PRJ-1
kbots agents start git --mode terminal
kbots agents start ops --mode headless --project api-service
kbots agents show AGT-1
kbots agents prompt AGT-1 "Review PR #42"
kbots agents interrupt AGT-1
kbots agents stop AGT-1
kbots project add /path/to/project
kbots project add --name "My Project"   # current directory
kbots project configure /path/to/project
kbots project configure --overwrite      # regenerate current project's OpenCode configs
kbots project sync                        # refresh starter AGENTS.md from embedded templates
kbots project sync --force                # also replace customs (backs them up first)
kbots sessions start --directory /path/to/project
kbots project list
kbots project show PROJECT-ID
kbots project watch --project PRJ-1   # prompt the git bot on file changes
kbots approve DEPLOY-23
kbots tui                              # minimal Codex-only terminal (default)
kbots tui --full                       # full 7-section operator dashboard
kbots gui
```

The `tasks` subcommands (`add`, `list`, `show`, `cancel`, `message`, `messages`,
`brief`, `inbox`, `update`) are all the same binary, so every one of them also works
via the `kb` alias (for example `kb tasks inbox`). `tasks message TASK-ID TEXT`
delivers a message to the task's CEO (or queues it when the CEO is busy);
`tasks messages TASK-ID` lists the task's messages, newest last. `tasks brief`
prints a global overview (generated time, task totals, and what needs action);
`tasks brief TASK-ID` prints one task's brief — status, CEO, summary, decisions,
blockers, next step, and pending message count. `tasks inbox` groups everything
needing action by kind (`approval`, `blocked`, `failed`, `queued`, `review`); an
empty inbox prints `Inbox clear.`. `tasks update TASK-ID --status STATUS [--note
TEXT]` moves a task to `review`, `blocked`, `done`, `failed`, or `canceled` —
the daemon validates the status and rejects terminal-state changes.

`agents list` shows the registry, saved session IDs, and live Herdr terminals. Managed
bots — spawned and tracked by the daemon with exact session binding — are started and
driven with:

```sh
kbots agents start git --mode terminal          # or --mode headless
kbots agents start git --mode headless --fresh  # new session instead of resuming
kbots agents show AGT-1
kbots agents prompt AGT-1 "Review PR #42"
kbots agents interrupt AGT-1                    # esc, session kept
kbots agents stop AGT-1                         # terminal killed, session kept
Start the git bot and its watcher is auto-provisioned (enabled, defaults) the
first time; an explicit `kbots project watch --project PRJ-1 --off` is kept and
never overridden by auto-provision. Use the watcher commands to change timing:

```sh
kbots project watch                              # list watchers
kbots project watch --project PRJ-1              # enable the file-change watcher
kbots project watch --project PRJ-1 --off        # disable
```
```

With the watcher enabled, the daemon polls `git status` and, once the tree is
quiet and the git agent is idle, prompts it with the changed paths — the bot
then reviews/acts per its project instructions. The daemon never commits or
pushes on its own.

Terminal mode spawns `opencode --session <id>` inside the role directory in a
Herdr-managed terminal; headless mode talks to the registered OpenCode service
only. Prompts are refused unless the agent is idle; `stop` keeps the OpenCode
session and history, `start` resumes the same session.

Use `--socket PATH` or `KBOTS_SOCKET` to select the daemon socket. By default,
the CLI uses `$XDG_RUNTIME_DIR/kbots.sock`, falling back to
`$HOME/.local/run/kbots.sock`. The GUI and TUI paths can be overridden with
`KBOTS_GUI_BIN` and `KBOTS_TUI_BIN`. Pass `--json` to force JSON output on a
terminal (default when piped).

The Bun daemon and JSON-RPC server exist (v1 minimal) and run as the
`kbots-daemon` systemd user service after `./install.sh` (opt out with
`KBOTS_SKIP_DAEMON_UNIT=1`; without systemd, `bun run daemon`). Until it
is running, `status`, `tasks`, `projects`, `agents`, and `approve` return a
clear connection error. `project add` prepares missing
`.agents/bot/{ceo,git,ops,router}` folders and starter `AGENTS.md` files before the
daemon request. Editable source templates live in `cli/templates/bots/` and
are embedded into the CLI binary at build time. Kbots creates a starter only
when that role’s `AGENTS.md` is missing; it never replaces existing bot
instructions during registration or configuration. If the project has no
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

`kbots project sync` refreshes the starter `.agents/bot/{ceo,git,ops,router}/AGENTS.md`
files from the templates embedded at build time. Missing files are created;
files identical to the template are left alone; customized files are kept unless
`--force` is passed, in which case each is backed up next to the original as
`AGENTS.md.bak` (then `AGENTS.md.bak.N`) before the template is written. The
project manifest and OpenCode configs are not touched.

`kbots sessions start` creates one idle OpenCode session for every enabled bot
in the manifest, using each role's `.agents/bot/<role>` directory so its
`opencode.jsonc` applies. It does not send a prompt or start agent work. Session
IDs are stored with mode `0600` under `${XDG_STATE_HOME:-~/.local/state}/kbots/projects/`
and reused on later runs when the OpenCode service still has them. The command
connects to or starts the local OpenCode service; it does not require the
workflow daemon.
The methods and newline-delimited JSON-RPC contract are documented
in [`../docs/kbots-ipc.md`](../docs/kbots-ipc.md).
