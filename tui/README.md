# Terminal UI

`control-center/` is the native Ratatui terminal client, using Crossterm for
keyboard input. It is designed to run inside Herdr, tmux, SSH, or Mosh; exiting
the UI restores the terminal and does not stop the backend or its agents.

The current dashboard is a demo scaffold with keyboard navigation (`↑/↓` or
`j`/`k`, `←/→` or `h`/`l`, or `1`–`6`) and `q`/`Esc` to quit. Below 112
columns it switches to a compact single-column view, hides the sidebar, shortens
the header, and stacks agents, queue, and events. Very short terminals show a
summary and a resize hint. It has no daemon connection yet. Backend state and
workflow decisions will come from the Bun daemon over the shared local IPC
protocol once that contract is implemented.

Run it with:

```sh
cargo run --manifest-path tui/control-center/Cargo.toml
```
