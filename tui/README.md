# Terminal UI

`control-center/` is the native Ratatui terminal client, using Crossterm for
keyboard input. It is designed to run inside Herdr, tmux, SSH, or Mosh; exiting
the UI restores the terminal and does not stop the backend or its agents.

The default TUI is a minimal Codex chat terminal: a one-line header
(`kbots · CODEX · LIVE/OFFLINE`) with a `✕ exit` button (or `Ctrl+]`) to quit,
and a fullscreen embedded Codex CLI below it — no other pages. Because the
Codex pane is the whole window, the TUI follows the Codex process out: when
Codex exits (running `exit`, or disconnecting), the final screen stays visible
for a short grace (~1.5 s) and the TUI then quits. `kb tui --full` (or
`--full` on `kbots-tui`) opens the complete operator dashboard described
below instead; there an exited Codex only marks the section `· exited`.

The dashboard connects to the kbots daemon over its local Unix socket
(JSON-RPC 2.0, one request per connection; discovery order `KBOTS_SOCKET` →
`$XDG_RUNTIME_DIR/kbots.sock` → `$HOME/.local/run/kbots.sock`). It renders
live data: a Codex page embedded as the first section, Overview counts
(tasks by status, managed agents, pending approvals, `system.status`),
Projects, Tasks (real rows, color-coded status), managed Agents grouped by
project (plus `herdr` state and `liveTerminals`), pending Approvals, and the
Activity log (`events.list`, newest first). Section navigation supports
arrows, `h/j/k/l`, `n/p`, and `1`–`7`.

The Codex page (section 1 of the Full layout) is a real interactive Codex CLI
owned by [herdr-core](https://github.com/ogulcancelik/herdr-core) (`hc`), the
same way every other kbots terminal is run. `hc` holds the PTY; the TUI is only
a viewer attached to it over `hc connect`'s framed JSON-RPC stream
(`terminal.attach`, then DATA/CREDIT/RESIZE/CLOSE frames). The terminal is
named and labelled by kbots (`kbots-codex-<task>-<scope>`, labels
`kbots.codex=1`, `kbots.scope=<cwd hash>`, `kbots.task=<binding>`,
`kbots.update_check=off`), runs
`codex --dangerously-bypass-approvals-and-sandbox -c
check_for_update_on_startup=false` (yolo mode; the startup update check is
disabled so the embedded chat cannot block on an update popup — updates are
managed outside the TUI) with its cwd
inside the project's `.agents/bot/router/` folder (the task-router
instructions for Codex). When the folder is missing, it is scaffolded from the
embedded template automatically (identical to `kb project sync`; never
overwrites existing files); if scaffolding fails, the terminal falls back to
the project root.

Every key except the host chord `Ctrl+]` goes to Codex, encoded from the
terminal's own modes: kitty keyboard-protocol flags (Codex pushes `CSI >7u`),
application cursor keys, and the mouse protocol. `Ctrl+]` returns to Overview.
The pane is bound to "free" or to one open task; `Ctrl+←`/`Ctrl+→` cycle the
binding, and the bound task's `TASK-id`/title is the spawn context. The binding
is stored as a `TASK-id` (list churn never re-points it). Switching the binding
detaches the current viewer and attaches the other task's terminal; both Codex
sessions keep running in `hc`, so switching no longer loses context.

The mouse wheel goes to Codex when it asked for mouse reports (it enables
`1000/1002/1003`), and to the local 1000-line scrollback otherwise;
`Ctrl+Shift+↑`/`Ctrl+Shift+↓` always scroll locally. `Ctrl+R` starts a fresh
Codex terminal when the attached one has exited, and re-attaches when it is
still live. A green `✕ exit codex` button at the top-right border of the Codex
pane leaves the page with one click (touch/Moshi friendly), alongside `Ctrl+]`.
Quitting kbots only detaches: Codex keeps running in herdr-core and can be
re-attached (by the TUI, `hc attach`, or the GUI) or inspected with `hc ls` and
`hc read`. Set `HC_BIN` to use an `hc` binary that is not on `PATH`. Under
Moshi/SSH everything is a keyboard, so the chords matter.
The dashboard polls every 5 seconds only while running, renders only on input or data
change, shows a persistent `daemon unreachable` banner (with inline errors
everywhere else) when the socket is gone, and keeps last-good data on
mid-session failures. Plain-text in the bottom composer is not routed to the
daemon anymore — use `@N <text>` to message task N directly, or `/brief`,
`/inbox`, `/tasks`, `/done N` and `/blocked N`. The natural-language router
(`router.run` in the daemon) is unaffected and still available over RPC.
v2 daemon methods missing
from the running daemon surface as `daemon not updated yet`, never a crash.
Section navigation supports arrows, `h/j/k/l`, `n/p`, and `1`–`7`. It also
supports Moshi-style gesture chords using the default `Ctrl+B` prefix: `n/p`
or `Shift+←/→` cycles sections, `w` opens a section navigator (`←/→` or
`↑/↓`, then Enter), Tab/Shift+Tab cycles the overview panes, and `z` zooms
the focused pane. Horizontal or vertical wheel events over the workspace
navigation chrome cycle sections; the TUI does not generate synthetic swipe
events. Mouse capture is disabled on exit and guarded by RAII during panics.
Below 112 columns the dashboard switches to a compact single-column view,
hides the sidebar, shortens the header, and stacks agents, queue, and
events. Very short terminals show a summary and a resize hint. Use `q` or
`Esc` to quit; type any printable character (except the `h j k l n p 1–7`
bindings) to focus the composer, `Tab` toggles it, `Esc` returns to navigation.

Run it with:

```sh
cargo run --manifest-path tui/control-center/Cargo.toml
```
