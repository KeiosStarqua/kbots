# Desktop GUI

`control-center/` is the native Slint application (Rust + Slint 1.19). It is a presentation and
operator-control client; workflow policy, persistence, and bot execution stay
in the Bun backend. The planned connection boundary is a local Unix socket
using JSON-RPC, with event notifications flowing from the daemon to the GUI.

The first screen is a dashboard scaffold with sample content. Live state,
commands, logs, approvals, and reconnect behavior will be added after the
backend API is defined.

Start the native app from this directory with:

```sh
cargo run --manifest-path control-center/Cargo.toml
```
