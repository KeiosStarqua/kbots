# Automation CLI

`kbots` is a Rust command-line client for automation. `kb` is a short
alias `install.sh` links to it (`~/.local/bin/kb` → `kbots`). It exposes stable
subcommands, then sends daemon-owned operations over the shared
`kbots-ipc` transport (Unix socket on Unix, authenticated TCP on Windows).
It does not implement orchestration or bypass daemon-side
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
kbots ag ls --all                  # include finished (stopped) agents in the Managed list
# Inside a project that has `.agents/bot/kbots.json` (written by `project add`),
# `ag ls` and `tasks ls` default to that project — no --project needed.
# A directory that only has `.agents/bot/` prompts `Add <path> to kbots? [y/N]`
# when stdin is a terminal. Yes scaffolds and registers it, then scopes the
# command. No, or a piped stdin, leaves the list unfiltered.
# Passing --project always wins. An unknown directory passed as --project
# gets the same prompt; declining prints the unknown-project error.
kbots prj ls          # `prj` for `project`, `ls` for `list`
kbots prj watch --project PRJ-1
kbots agents start git
kbots agents start ops --project api-service
kbots agents show AGT-1
kbots ag logs AGT-1                       # session history + live terminal screen
kbots agents prompt AGT-1 "Review PR #42"
kbots agents interrupt AGT-1
kbots agents stop AGT-1
kbots project add /path/to/project
kbots project add --name "My Project"   # current directory
kbots project configure /path/to/project
kbots project configure --overwrite      # regenerate current project's OpenCode configs
kbots project sync                        # upgrade starter AGENTS.md from the embedded templates
kbots project sync --check                # report stale/customized files; write nothing
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
kbots agents start git                           # hc terminal, same as default for task CEOs
kbots agents start git --fresh                  # new session instead of resuming
kbots agents show AGT-1
kbots ag logs AGT-1                       # session history + live terminal screen
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

`agents start` spawns `opencode --session <id>` inside the role directory in a
Herdr-managed terminal (the only mode; headless was retired 2026-11-19).
Prompts are refused unless the agent is idle; `stop` keeps the OpenCode
session and history, `start` resumes the same session.

### What each bot is doing, and what is wrong

`status`, `agents list`, and `agents show` end with a bot block derived by the
daemon (`src/backend/bots/health.ts`). It answers two questions without
guessing from process names:

```text
bots      3 (2 ok, 1 warning, 0 error, 0 stopped, 0 waiting)
          ⚠ AGT-7 ceo · TASK-7: idle while task TASK-7 is running
```

`agents list` prints one line per managed bot — health glyph, activity
(`working · turn 7`, `idle · turn 3`, `blocked`), and the task it is bound to —
then the problem detail plus a hint for the codes that need action.

```text
bots
  ✔ AGT-1 git  · no task idle · turn 45
  ⚠ AGT-7 ceo  · TASK-7 running idle · turn 3
      idle_with_open_task idle while task TASK-7 is running
      → the nudger kicks this task automatically (KBOTS_NUDGE)
```

The daemon also acts on these on its own: a CEO terminal whose task is
finished is reaped (session kept), a CEO that crashed while its task is still
open is resumed against the same session, and an old task whose CEO stopped
outside `KBOTS_RECOVER_WINDOW_MS` is continued by the stale-task step instead
of staying in `researching` forever. Both resumes share the budget
`KBOTS_RECOVER_MAX` (default 2) and `KBOTS_RECOVER_COOLDOWN_MS` (default 5
min). `KBOTS_REAP=off` / `KBOTS_RECOVER=off` / `KBOTS_STALE_CONTINUE=off`
disable those steps. Sessions are never deleted.


Use `--socket PATH` / `KBOTS_SOCKET` for a Unix daemon, or `--host HOST:PORT` /
`KBOTS_HOST` for a TCP daemon (the only transport on Windows). By default the
CLI uses `$XDG_RUNTIME_DIR/kbots.sock`, falling back to
`$HOME/.local/run/kbots.sock` on Unix and `127.0.0.1:7878` on Windows. `kbots
connect --host 100.x.x.x:7878` saves a remote daemon for later commands;
`kbots connect --local` clears it. TCP requests use the token in
`$HOME/.local/state/kbots/token`, overridable with `--token` / `KBOTS_TOKEN`.
The GUI and TUI paths can be overridden with `KBOTS_GUI_BIN` and
`KBOTS_TUI_BIN`. Pass `--json` to force JSON output on a terminal (default when
piped).

On Windows, `kbots daemon start|stop|status` manage a detached local daemon
using a pid file in the state dir — no service and no admin rights. Unix hosts
run the `kbots-daemon` systemd user service after `./install.sh` (opt out with
`KBOTS_SKIP_DAEMON_UNIT=1`; without systemd, `bun run daemon`). Until it
is running, `status`, `tasks`, `projects`, `agents`, and `approve` return a
clear connection error. `project add` prepares missing
`.agents/bot/{ceo,git,ops,audit,router}` folders and starter `AGENTS.md` files before the
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

Projects that already have per-bot OpenCode configs are adopted instead of
refused: `project add` / `project configure` read each existing
`.agents/bot/<role>/opencode.jsonc` (JSONC — comments and trailing commas are
fine), derive `.agents/bot/kbots.json` from them (the project-specific
`$schema` is dropped; roles without a config get the starter defaults), and
leave the config files byte-identical. Project-specific instructions and
plugins stay local instead of being copied between repositories. Use
`--overwrite` afterwards only if you want the configs regenerated from the
new manifest.

MCP servers are declared globally in OpenCode, so every session in the
project would otherwise load all of them. A role can switch individual
servers off by naming them under `mcp.disable` — no need to restate the
server command:

```json
{
  "bots": {
    "ceo": {
      "enabled": true,
      "opencode": {
        "instructions": [".agents/bot/ceo/AGENTS.md"],
        "plugins": [],
        "mcp": { "disable": ["rea"] }
      }
    }
  },
  "version": 1
}
```

`kbots project configure` reads each named server from the global OpenCode
config (`mcp.servers.<name>` in v2, the legacy `mcp.<name>` shape otherwise)
and writes it into that role's `opencode.jsonc` with `disabled: true`. Only
`type`, `command`, `cwd`, `url`, and `timeout` are copied; `environment`,
`headers`, and `oauth` stay out of the generated file, so a tracked project
config never carries a secret. An unknown server name, or a name the role
already defines under `mcp.servers`, is an error. This affects that role's
sessions only (they run in `.agents/bot/<role>`), not sessions opened at the
project root. Existing configs are only rewritten by `--overwrite`.

`kbots project sync` refreshes the starter `.agents/bot/{ceo,git,ops,audit,router}/AGENTS.md`
files from the templates embedded at build time. Missing files are created;
files identical to the template are left alone; customized files are kept unless
`--force` is passed, in which case each is backed up next to the original as
`AGENTS.md.bak` (then `AGENTS.md.bak.N`) before the template is written. The
project manifest and OpenCode configs are not touched; the only other write is
the stamp's git-ignore entry (below).

### Per-repo scaffolding version

kbots is installed once per machine (the CLI, the clients, and one daemon);
each registered repo carries its own copy of the scaffolding. To keep those
copies manageable across kbots upgrades, seeding writes a local stamp at
`.agents/bot/.scaffold.json`:

```json
{ "kbotsVersion": "2026.11.20.8", "templatesHash": "…", "files": { "git": "…" } }
```

- `kbotsVersion` is the release that last wrote the scaffolding (the deploy
  version embedded by `./install.sh`; `0.1.0` for a dev build).
- The stamp is machine-local state, so seeding appends `.scaffold.json` to
  `.agents/bot/.gitignore` (creating that file when missing); it never appears
  as an untracked change.
- `files` records the content hash of every file kbots wrote. After a kbots
  upgrade, `kbots project sync` **upgrades files whose bytes are unchanged
  since the recorded sync**, keeps customized files, and reports `previous`
  → new version. Only `--force` overwrites a customized file (with a backup).
- `kbots project sync --check` reports `stale` (upgradeable) and `customized`
  files without writing; `kbots prj ls` shows each repo's scaffold version
  and a `(stale)` marker when the template set changed.
- A repo scaffolded before this stamp existed has no recorded hashes, so it
  is treated as customized: migrate it once with
  `kbots project sync --force` (backups are kept), after which upgrades are
  automatic for untouched files.
- Nothing updates repo files by itself when the global kbots is upgraded —
  run `kbots project sync` once after installing a new version
  (`kbots prj ls` tells you which repos need it).

`kbots sessions start` creates one idle OpenCode session for every enabled bot
in the manifest, using each role's `.agents/bot/<role>` directory so its
`opencode.jsonc` applies. It does not send a prompt or start agent work. Session
IDs are stored with mode `0600` under `${XDG_STATE_HOME:-~/.local/state}/kbots/projects/`
and reused on later runs when the OpenCode service still has them. The command
connects to or starts the local OpenCode service; it does not require the
workflow daemon.
The methods and newline-delimited JSON-RPC contract are documented
in [`../docs/kbots-ipc.md`](../docs/kbots-ipc.md).
