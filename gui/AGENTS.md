# Desktop GUI

## Purpose

- Native [Slint](https://slint.dev) desktop application: the operator-control client. Workflow policy, persistence, and bot execution stay in the Bun backend; the GUI talks to it through the shared `../ipc/` transport (Unix socket on Unix, authenticated TCP on Windows/remote), documented in `../docs/kbots-ipc.md` and shared with the [Ratatui TUI](../tui/README.md).

## Ownership

- Owns this directory: `main.slint`, `build.rs`, `src/` (including `src/terminal/` for Herdr Core attach), `kbots.desktop.in`, and packaging for the desktop entry.

## Local Contracts

- The GUI is an operator-control client only; it must remain safe to close while jobs continue running. Session and workflow ownership remain in the daemon.
- The Terminals page discovers live terminals through the Herdr Core framed JSON-RPC socket and attaches directly to an existing PTY. It does not start `hc attach`, another terminal process, or a second PTY. The VT screen is parsed with `vt100` and rendered as colored Slint cells; keyboard input, focus, and window-driven resizes are sent over the attached stream. Discovery and attachment are limited to live terminals the daemon reports inside registered projects.
- Set `HC_SOCKET` to override the Herdr Core socket; otherwise the GUI uses `$XDG_RUNTIME_DIR/herdr-core/default.sock`, then `/run/user/<uid>/herdr-core/default.sock`. On Windows Herdr Core is unavailable unless `HC_SOCKET=tcp://HOST:PORT`; the Terminals page then reports that state instead of crashing.
- IPC discovery and method shapes are owned by `../docs/kbots-ipc.md`; do not invent client-local variants.

## Work Guidance

- Keep UI work aligned with the Apple HIG principles in the root `AGENTS.md` (clear feedback, reversible actions, consistent interactions, accessibility first) where they apply to a native desktop surface; do not copy Apple-only conventions that conflict with the host platform.
- Icons come from Phosphor Icons only (root `AGENTS.md`).
- Keep daemon-unreachable and v2-method-missing states as visible, non-crashing UI (`daemon not updated yet`-style), never as panics.

## Verification

- `cargo check` in this directory; run the app with `cargo run --manifest-path Cargo.toml` from here.
- `./install.sh` from the repo root builds and installs the GUI and deploys the daemon on Unix hosts; `powershell -ExecutionPolicy Bypass -File .\install.ps1` is the Windows equivalent (Start Menu shortcut included); on Nix-managed hosts `kbdeploy "what changed"` is the one deploy command (`nix build .#` is the packaging check).

## Child DOX Index

None.
