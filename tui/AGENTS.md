# Terminal UI

## Purpose

- `tui/control-center/` is the native Ratatui terminal client, using Crossterm for keyboard input. It is designed to run inside Herdr, tmux, SSH, or Mosh; exiting the UI restores the terminal and does not stop the backend or its agents.

## Ownership

- Owns `tui/control-center/` (Rust crate, `src/main.rs`, `src/rpc.rs`). Workflow policy, persistence, and bot execution stay in the Bun backend.

## Local Contracts

- Connects to the kbots daemon over its local Unix socket (JSON-RPC 2.0, one request per connection; discovery order `KBOTS_SOCKET` → `$XDG_RUNTIME_DIR/kbots.sock` → `$HOME/.local/run/kbots.sock`). The contract is `../docs/kbots-ipc.md`.
- Renders live data only: Overview counts, Tasks, Projects, managed Agents (with `herdr` state and `liveTerminals`), pending Approvals, the Activity log (`events.list`, newest first), and the Codex chat page. The Codex page is a chat-style session against the daemon's natural-language router (`router.run`): every message gets one bounded, read-only Codex turn, or a deterministic `@TASK-n` bypass. The transcript is session-local, in memory; persistence stays in the daemon's event log (`router.decision`). Polls every 5 seconds only while running; renders only on input or data change; keeps last-good data on mid-session failures and shows a persistent `daemon unreachable` banner when the socket is gone.
- The bottom composer routes plain text through the daemon's natural-language router (`router.run`); ambiguous requests show numbered candidates (1–9 to pick, Esc to dismiss, or retype with `@TASK-id`). `@N <text>` messages task N directly; `/brief`, `/inbox`, `/tasks`, `/done N`, `/blocked N` work as before.
- Section navigation: arrows, `h/j/k/l`, `n/p`, `1`–`7`, plus Moshi-style gesture chords behind the `Ctrl+B` prefix; wheel cycles sections. Below 112 columns the dashboard switches to a compact single-column view; very short terminals show a summary and a resize hint.
- It must restore the terminal on normal exit and on panics (RAII guards; mouse capture disabled on exit). v2 daemon methods missing from the running daemon surface as `daemon not updated yet`, never a crash.

## Work Guidance

- Keep the dashboard thin: no client-side workflow decisions, retries beyond last-good caching, or state the daemon owns.
- Keep `q`/`Esc` quit behavior, composer behavior, and terminal restoration intact across changes; test in a real terminal before shipping.
- See `README.md` for run commands and full keybindings.

## Verification

- `cargo check` in `tui/control-center/`.
- `./install.sh` from the repo root builds and installs the TUI and deploys the daemon; the TUI must be running freshly deployed code.

## Child DOX Index

None.
