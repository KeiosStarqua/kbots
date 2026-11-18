# Desktop GUI

This directory contains the native Slint desktop application. It is an
operator-control client; workflow policy, persistence, and bot execution stay
in the Bun backend. It connects through the local Unix socket JSON-RPC contract
documented in [kbots IPC](../docs/kbots-ipc.md), shared with the
[Ratatui TUI](../tui/README.md).

The GUI provides status, project registration and lookup, task creation/listing/
inspection/cancellation, agent and approval views, approval actions, recent
activity, and embedded interactive Herdr Core terminals. The Terminals page
discovers live terminals through the Herdr Core framed JSON-RPC socket and
attaches directly to an existing PTY. It does not start `hc attach`, another
terminal process, or a second PTY. The VT screen is parsed with `vt100` and
rendered as colored Slint cells; keyboard input, focus, and window-driven
resizes are sent over the attached stream. Page Up and Page Down browse local
VT scrollback. Attached terminals remain available as tabs, and "Observe only"
creates a read-only attachment. Discovery and attachment are limited to live
terminals the daemon reports inside registered projects. Session and workflow
ownership remain in the daemon.

Set `HC_SOCKET` to override the Herdr Core socket. Otherwise the GUI uses
`$XDG_RUNTIME_DIR/herdr-core/default.sock`, then
`/run/user/<uid>/herdr-core/default.sock`.

Start the native app from this directory with:

```sh
cargo run --manifest-path Cargo.toml
```
