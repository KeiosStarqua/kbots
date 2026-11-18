# Terminal UI

`control-center/` is the native Ratatui terminal client, using Crossterm for
keyboard input. It is designed to run inside Herdr, tmux, SSH, or Mosh; exiting
the UI restores the terminal and does not stop the backend or its agents.

The dashboard connects to the kbots daemon over its local Unix socket
(JSON-RPC 2.0, one request per connection; discovery order `KBOTS_SOCKET` →
`$XDG_RUNTIME_DIR/kbots.sock` → `$HOME/.local/run/kbots.sock`). It renders
live data: Overview counts (tasks by status, managed agents, pending
approvals, `system.status`), Tasks (real rows, color-coded status), Projects,
managed Agents grouped by project (plus `herdr` state and `liveTerminals`),
pending Approvals, and the Activity log (`events.list`, newest first). It
polls every 5 seconds only while running, renders only on input or data
change, shows a persistent `daemon unreachable` banner (with inline errors
everywhere else) when the socket is gone, and keeps last-good data on
mid-session failures. A bottom composer creates tasks from plain text, routes
`@N <text>` to task N's CEO (`tasks.message`), and implements `/brief`,
`/inbox`, `/tasks`, `/done N` and `/blocked N`. v2 daemon methods missing
from the running daemon surface as `daemon not updated yet`, never a crash.
Section navigation supports arrows, `h/j/k/l`, `n/p`, and `1`–`6`. It also
supports Moshi-style gesture chords using the default `Ctrl+B` prefix: `n/p`
or `Shift+←/→` cycles sections, `w` opens a section navigator (`←/→` or
`↑/↓`, then Enter), Tab/Shift+Tab cycles the overview panes, and `z` zooms
the focused pane. Horizontal or vertical wheel events over the workspace
navigation chrome cycle sections; the TUI does not generate synthetic swipe
events. Mouse capture is disabled on exit and guarded by RAII during panics.
Below 112 columns the dashboard switches to a compact single-column view,
hides the sidebar, shortens the header, and stacks agents, queue, and
events. Very short terminals show a summary and a resize hint. Use `q` or
`Esc` to quit; type any printable character (except the `h j k l n p 1–6`
bindings) to focus the composer, `Tab` toggles it, `Esc` returns to navigation.

Run it with:

```sh
cargo run --manifest-path tui/control-center/Cargo.toml
```
