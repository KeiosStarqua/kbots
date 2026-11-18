# Desktop GUI

This directory contains the native Slint desktop application. It is an
operator-control client; workflow policy, persistence, and bot execution stay
in the Bun backend. It connects through the local Unix socket JSON-RPC contract
documented in [kbots IPC](../docs/kbots-ipc.md), shared with the
[Ratatui TUI](../tui/README.md).

The GUI provides status, project registration and lookup, task creation/listing/
inspection/cancellation, agent and approval views, approval actions, recent
activity, and socket configuration guidance. Results and connection failures
are shown in the app.

Start the native app from this directory with:

```sh
cargo run --manifest-path Cargo.toml
```
