# Terminal UI

`control-center/` is the native Ratatui terminal client, using Crossterm for
keyboard input. It is designed to run inside Herdr, tmux, SSH, or Mosh; exiting
the UI restores the terminal and does not stop the backend or its agents.

The current dashboard is a demo scaffold with keyboard navigation (`↑/↓`,
`j`/`k`, or `1`–`6`) and `q`/`Esc` to quit. It has no daemon connection yet.
Backend state and workflow decisions will come from the Bun daemon over the
shared local IPC protocol once that contract is implemented.

Run it with:

```sh
cargo run --manifest-path tui/control-center/Cargo.toml
```
