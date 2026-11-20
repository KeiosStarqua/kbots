# Terminal UI

## Purpose

- `tui/control-center/` is the native Ratatui terminal client, using Crossterm for keyboard input. It is designed to run inside Herdr, tmux, SSH, or Mosh; exiting the UI restores the terminal and does not stop the backend or its agents. The default layout is a minimal Codex-only terminal (one-line header with live state and a `✕ exit` button; `Ctrl+]` quits; the embedded Codex CLI fills the rest). `--full` opens the complete 7-section dashboard instead.

## Ownership

- Owns `tui/control-center/` (Rust crate, `src/main.rs`, `src/rpc.rs`). Workflow policy, persistence, and bot execution stay in the Bun backend.

## Local Contracts

- Connects to the kbots daemon over its local Unix socket (JSON-RPC 2.0, one request per connection; discovery order `KBOTS_SOCKET` → `$XDG_RUNTIME_DIR/kbots.sock` → `$HOME/.local/run/kbots.sock`). The contract is `../docs/kbots-ipc.md`.
- Renders live data only: a Codex page embedded as section 1, Overview counts, Tasks, Projects, managed Agents (with `herdr` state and `liveTerminals`), pending Approvals, and the Activity log (`events.list`, newest first). Polls every 5 seconds only while running; renders only on input or data change; keeps last-good data on mid-session failures and shows a persistent `daemon unreachable` banner when the socket is gone.
- The Codex page (section 1 of the Full layout) shows a real Codex CLI owned by herdr-core (`hc`), the same runtime that holds every other kbots terminal. The TUI does not spawn a PTY: it runs `hc connect`, finishes the framed JSON-RPC handshake (`hello {framed:true}`), finds the live terminal labelled `kbots.codex=1` + `kbots.scope=<cwd hash>` + `kbots.task=<binding>` (or starts `codex --dangerously-bypass-approvals-and-sandbox` in a new one named `kbots-codex-<task>-<scope>`), and attaches with its pane size. First paint and live output are raw VT bytes fed to `vt100` and rendered by `tui-term`; every DATA frame is credited; Drop sends CLOSE and never kills the program. `HC_BIN` overrides the `hc` binary.
- Codex's cwd is the project's `.agents/bot/router/` folder (Codex task-router instructions); when missing, the folder is scaffolded from the embedded template automatically (same content as `kb project sync`; never overwrites); falls back to the project root only on scaffold failure. Codex runs in yolo mode (owner directive — never asks for permission).
- Everything except the host chord `Ctrl+]` is forwarded to the program, encoded from the terminal's own modes: kitty keyboard-protocol flags tracked from its output (Codex pushes `CSI >7u`), application cursor keys, and the mouse protocol. `Ctrl+]` returns to Overview, as does clicking the green `✕ exit codex` button at the top-right border of the pane. `Ctrl+←`/`Ctrl+→` cycle the binding (free session ↔ open tasks, in list order); switching detaches the current viewer and attaches the other task's terminal, which keeps running in `hc`. `Ctrl+Shift+↑`/`Ctrl+Shift+↓` scroll the 1000-line local scrollback; the wheel goes to Codex when it asked for mouse reports, else to that scrollback. `Ctrl+R` starts a fresh Codex terminal when the attached one exited. The transcript/session is not persisted by kbots, but the terminal is persisted by `hc`; `router.run` remains available in the daemon (used by neither the TUI nor documented callers now).
- The bottom composer does not call the router: `@N <text>` messages task N directly; `/brief`, `/inbox`, `/tasks`, `/done N`, `/blocked N` work as before; unknown plain text shows a hint, not an RPC.
- Section navigation (Full layout): arrows, `h/j/k/l`, `n/p`, `1`–`7` (`1` = Codex), plus Moshi-style gesture chords behind the `Ctrl+B` prefix; wheel cycles sections off the Codex page (on the Codex page it goes to the attached program or its scrollback). Below 112 columns the dashboard switches to a compact single-column view; very short terminals show a summary and a resize hint.
- It must restore the terminal on normal exit and on panics (RAII guards; mouse capture disabled on exit). v2 daemon methods missing from the running daemon surface as `daemon not updated yet`, never a crash.
- Simple mode follows the attached Codex terminal: when the program ends (the core sends CLOSE), the final screen stays visible for a short grace (~1.5 s) and the TUI quits (same effect as `Ctrl+]`). `--full` keeps the dashboard open and only marks the Codex section `· exited`.
- The Codex pane is one router terminal: a single pty at a time. The binding stores a `TASK-id` (not a list index, so task churn cannot re-point it), and switching the binding closes the previous Codex process and spawns the new context. Never keep a pty per task.

## Work Guidance

- Keep the dashboard thin: no client-side workflow decisions, retries beyond last-good caching, or state the daemon owns.
- Keep `q`/`Esc` quit behavior, terminal restoration, and the Codex page chords (`Ctrl+]`, `Ctrl+Left/Right`, `Ctrl+R`) intact across changes; test in a real terminal before shipping. `src/codex_term.rs` owns the hc connection, frame codec, attach/detach lifecycle, render, key/mouse encoding, and scrollback — keep it self-contained. Never replace the attach stream with a locally spawned PTY.
- See `README.md` for run commands and full keybindings.

## Verification

- `cargo check` in `tui/control-center/`.
- `cargo test` in `tui/control-center/` covers the frame codec, kitty flag tracking, key/mouse encoding, and terminal naming.
- Live check: run the TUI in a real terminal, confirm `hc ls` shows `kbots-codex-<task>-<scope>`, type into Codex, quit with `Ctrl+]`, then confirm the terminal is still `live` (`hc read <address>`).
- `./install.sh` from the repo root builds and installs the TUI and deploys the daemon; the TUI must be running freshly deployed code.

## Child DOX Index

None.
