# Terminal UI

`control-center/` is the native Ratatui terminal client, using Crossterm for
keyboard input. It is designed to run inside Herdr, tmux, SSH, or Mosh; exiting
the UI restores the terminal and does not stop the backend or its agents.

The current dashboard is a demo scaffold. Section navigation supports arrows,
`h/j/k/l`, `n/p`, and `1`–`6`. It also supports Moshi-style gesture chords using
the default `Ctrl+B` prefix: `n/p` or `Shift+←/→` cycles sections, `w` opens a
section navigator (`←/→` or `↑/↓`, then Enter), Tab/Shift+Tab cycles the
overview panes, and `z` zooms the focused pane. Horizontal or vertical wheel
events over the workspace navigation chrome cycle sections; the TUI does not
generate synthetic swipe events. Mouse capture is disabled on exit and guarded
by RAII during panics. Use `q` or `Esc` to quit.

Below 112 columns the dashboard switches to a compact single-column view,
hides the sidebar, shortens the header, and stacks agents, queue, and events.
Very short terminals show a summary and a resize hint. It has no daemon
connection yet. Backend state and workflow decisions will come from the Bun
daemon over the shared local IPC protocol once that contract is implemented.

Run it with:

```sh
cargo run --manifest-path tui/control-center/Cargo.toml
```
