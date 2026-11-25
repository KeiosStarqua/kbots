# Shared IPC transport

## Purpose

- `ipc/` is the Rust crate `kbots-ipc`: one newline-delimited JSON-RPC 2.0
  request/response per connection over a Unix socket (Unix hosts) or an
  authenticated TCP listener (Windows hosts and remote/Tailscale clients).
  CLI, TUI and GUI use it so discovery and auth cannot drift apart.

## Ownership

- Owns `Cargo.toml` and `src/lib.rs`. It owns no workflow state, no daemon
  behavior, and no UI; the Bun daemon (`../src/backend/daemon.ts`) owns handlers
  and the token file, and `../docs/kbots-ipc.md` owns the wire contract.

## Local Contracts

- Discovery order: `--host`/`KBOTS_HOST` → `--socket`/`KBOTS_SOCKET` →
  saved host from `kbots connect` → platform default (`$XDG_RUNTIME_DIR` then
  `$HOME/.local/run` on Unix; `127.0.0.1:7878` on Windows). An explicit socket
  or host means exactly that one endpoint.
- TCP requests carry a top-level `token` from `KBOTS_TOKEN` or
  `$HOME/.local/state/kbots/token`; Unix socket requests do not. The daemon
  returns JSON-RPC `-32001` for a missing/invalid token.
- State/config lives in `$HOME/.local/state/kbots` (`USERPROFILE` fallback on
  Windows): `token` (daemon-generated secret), `client.json` (`host`,
  `repoRoot`), `daemon.pid` (CLI-managed daemon process).
- Client config is inert when unreadable or malformed; never surface a token
  value in logs or UI.

## Work Guidance

- Add transport features here, not in client crates. Clients may own flags and
  rendering, but not framing, endpoints, or auth.
- Keep Unix sockets as the default on Unix for backward compatibility; do not
  bind AF_UNIX on Windows.
- `Stream::connect_unix_or_host` is for Herdr Core attachment: a path on Unix,
  `tcp://HOST:PORT` on Windows.

## Verification

- `cargo check --manifest-path ipc/Cargo.toml` (Rust), plus `cargo check` in
  `cli/`, `tui/control-center/`, and `gui/` when the API changes.
- `bun run tsc --noEmit` when the daemon-side token/transport changes.

## Child DOX Index

None.
