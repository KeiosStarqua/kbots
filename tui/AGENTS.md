# Terminal UI

## Purpose

- `tui/control-center/` is the native Ratatui terminal client, using Crossterm for keyboard input. It is designed to run inside Herdr, tmux, SSH, or Mosh; exiting the UI restores the terminal and does not stop the backend or its agents. The default layout is a minimal Codex-only terminal (one-line header with live state and a `✕ exit` button; `Ctrl+]` quits; the embedded Codex CLI fills the rest). `--full` opens the complete 7-section dashboard instead.

## Ownership

- Owns `tui/control-center/` (Rust crate, `src/main.rs`, `src/rpc.rs`). Workflow policy, persistence, and bot execution stay in the Bun backend.

## Local Contracts

- Connects to the kbots daemon over its local Unix socket (JSON-RPC 2.0, one request per connection; discovery order `KBOTS_SOCKET` → `$XDG_RUNTIME_DIR/kbots.sock` → `$HOME/.local/run/kbots.sock`). The contract is `../docs/kbots-ipc.md`.
- Renders live data only: a Codex page embedded as section 1, Overview counts, Tasks, Projects, managed Agents (with `herdr` state and `liveTerminals`), pending Approvals, and the Activity log (`events.list`, newest first). Polls every 5 seconds only while running; renders only on input or data change; keeps last-good data on mid-session failures and shows a persistent `daemon unreachable` banner when the socket is gone.
- The Codex page (section 1 of the Full layout) embeds a real Codex CLI as a pseudo-terminal (`portable-pty` → `vt100` → `tui-term`, spawned as `codex --dangerously-bypass-approvals-and-sandbox` (yolo mode, owner directive — never asks for permission), cwd = the project's `.agents/bot/router/` folder (Codex task-router instructions); when missing, the folder is scaffolded from the embedded template automatically (same content as `kb project sync`; never overwrites); falls back to the project root only on scaffold failure). Everything except the host chord `Ctrl+]` is forwarded to the process; `Ctrl+]` returns to Overview, as does clicking the green `✕ exit codex` button at the top-right border of the pane. `Ctrl+←`/`Ctrl+→` cycle the binding (free session ↔ open tasks, in list order); one pty per bound task stays alive in memory so switching loses no context; quitting the TUI kills the children. `Ctrl+Shift+↑`/`Ctrl+Shift+↓` and the mouse wheel scroll the 1000-line scrollback; `Ctrl+R` respawns an exited process. The transcript/session is not persisted by kbots; `router.run` remains available in the daemon (used by neither the TUI nor documented callers now).
- The bottom composer does not call the router: `@N <text>` messages task N directly; `/brief`, `/inbox`, `/tasks`, `/done N`, `/blocked N` work as before; unknown plain text shows a hint, not an RPC.
- Section navigation (Full layout): arrows, `h/j/k/l`, `n/p`, `1`–`7` (`1` = Codex), plus Moshi-style gesture chords behind the `Ctrl+B` prefix; wheel cycles sections off the Codex page (on the Codex page it scrolls the pty scrollback). Below 112 columns the dashboard switches to a compact single-column view; very short terminals show a summary and a resize hint.
- It must restore the terminal on normal exit and on panics (RAII guards; mouse capture disabled on exit). v2 daemon methods missing from the running daemon surface as `daemon not updated yet`, never a crash.

## Work Guidance

- Keep the dashboard thin: no client-side workflow decisions, retries beyond last-good caching, or state the daemon owns.
- Keep `q`/`Esc` quit behavior, terminal restoration, and the Codex page chords (`Ctrl+]`, `Ctrl+Left/Right`, `Ctrl+R`) intact across changes; test in a real terminal before shipping. `src/codex_term.rs` owns pty spawn, render, key encoding, and scrollback — keep it self-contained.
- See `README.md` for run commands and full keybindings.

## Verification

- `cargo check` in `tui/control-center/`.
- `./install.sh` from the repo root builds and installs the TUI and deploys the daemon; the TUI must be running freshly deployed code.

## Child DOX Index

None.
