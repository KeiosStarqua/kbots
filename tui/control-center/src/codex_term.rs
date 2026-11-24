//! Embedded Codex terminal, owned by herdr-core (`hc`).
//!
//! Section 1 of the TUI used to spawn `codex` as a child of the TUI itself, so
//! quitting kbots killed it and switching the task binding lost its context.
//! It now works the way herdr-core runs terminals: `hc` owns the PTY, and this
//! module is only a *viewer* attached to it over `hc connect`'s framed
//! JSON-RPC stream (`[channel u32 LE][kind u8][len u32 LE][payload]`).
//!
//! Codex runs in a terminal named and labelled by kbots, so:
//! - detaching (quitting the TUI, switching the binding) leaves Codex alive;
//! - coming back re-attaches to the same terminal with its screen history;
//! - `hc ls`, `hc read`, and `hc attach` see it like any other terminal.
//!
//! What the viewer does, following herdr-core's contract:
//! - attach with its pane size and first paint comes back as raw VT bytes,
//!   fed to a `vt100` parser and rendered by `tui-term`;
//! - every DATA frame is credited so the core's flow control stays open;
//! - keys are encoded from the terminal's own modes (kitty keyboard flags,
//!   application cursor keys, mouse protocol) instead of fixed ANSI;
//! - the wheel goes to the program when it asked for mouse reports, and to
//!   the local scrollback otherwise;
//! - dropping the viewer sends CLOSE and does not kill the program.

use std::{
    collections::HashMap,
    env,
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders, BorderType},
};
use serde_json::{Value, json};
use tui_term::{vt100, widget::PseudoTerminal};

/// Number of scrollback lines kept per attached terminal.
const SCROLLBACK_LINES: usize = 1_000;

/// Framed-mode frame kinds (see herdr-core `schema/README.md`).
const DATA: u8 = 0;
const CREDIT: u8 = 1;
const CLOSE: u8 = 2;
const RESIZE: u8 = 3;
const HEADER: usize = 9;
const MAX_PAYLOAD: usize = 16 << 20;

/// Starter instructions for a project's embedded Codex task-router (kept
/// in sync with `cli/templates/bots/router/AGENTS.md`; this is the same
/// file embedded into the CLI).
const ROUTER_TEMPLATE: &str = include_str!("../../../cli/templates/bots/router/AGENTS.md");

/// Create `<root>/.agents/bot/router/AGENTS.md` from the embedded template
/// when missing. Idempotent and never overwrites an existing file.
pub fn scaffold_router_folder(git_root: &Path) -> io::Result<PathBuf> {
    let dir = git_root.join(".agents").join("bot").join("router");
    std::fs::create_dir_all(&dir)?;
    let agents = dir.join("AGENTS.md");
    if !agents.exists() {
        std::fs::write(&agents, ROUTER_TEMPLATE)?;
    }
    Ok(dir)
}

/// What the reader thread learns about the attached program.
#[derive(Default)]
struct ViewerState {
    /// Why the program or the connection ended, once it has.
    exit: Option<String>,
}

/// The kitty keyboard protocol flags and stack, tracked from the raw stream
/// because `vt100` does not model them. Keys are encoded as kitty asks only
/// when the program pushed flags (`codex` pushes 7) or set them.
#[derive(Default)]
struct Kitty {
    stack: Vec<u16>,
    scan: Scan,
}

#[derive(Default)]
struct Scan {
    state: ScanState,
    /// CSI prefix (`>`, `<`, `=`), then its parameter bytes.
    prefix: Option<u8>,
    params: Vec<u8>,
}

#[derive(Default, PartialEq)]
enum ScanState {
    #[default]
    Ground,
    Esc,
    Csi,
    Osc,
    OscEsc,
}

impl Kitty {
    fn flags(&self) -> u16 {
        self.stack.last().copied().unwrap_or(0)
    }

    /// Consume bytes looking for the keyboard-mode sequences, tolerating
    /// sequences split across reads.
    fn scan(&mut self, bytes: &[u8]) {
        for &b in bytes {
            let state = std::mem::replace(&mut self.scan.state, ScanState::Ground);
            self.scan.state = match (state, b) {
                (ScanState::Ground, 0x1b) => ScanState::Esc,
                (ScanState::Ground, _) => ScanState::Ground,
                (ScanState::Esc, b'[') => {
                    self.scan.prefix = None;
                    self.scan.params.clear();
                    ScanState::Csi
                }
                (ScanState::Esc, b']' | b'P' | b'_') => ScanState::Osc,
                (ScanState::Esc, 0x1b) => ScanState::Esc,
                (ScanState::Esc, _) => ScanState::Ground,
                (ScanState::Csi, b @ (b'<' | b'>' | b'=')) if self.scan.prefix.is_none() => {
                    self.scan.prefix = Some(b);
                    ScanState::Csi
                }
                (ScanState::Csi, b'0'..=b'9' | b';') => {
                    if self.scan.params.len() < 32 {
                        self.scan.params.push(b);
                    }
                    ScanState::Csi
                }
                (ScanState::Csi, b'u') => {
                    self.apply();
                    ScanState::Ground
                }
                (ScanState::Csi, 0x20..=0x7e) => ScanState::Ground,
                (ScanState::Csi, 0x1b) => ScanState::Esc,
                (ScanState::Csi, _) => ScanState::Csi,
                (ScanState::Osc, 0x07) => ScanState::Ground,
                (ScanState::Osc, 0x1b) => ScanState::OscEsc,
                (ScanState::Osc, _) => ScanState::Osc,
                (ScanState::OscEsc, b'\\') => ScanState::Ground,
                (ScanState::OscEsc, 0x1b) => ScanState::OscEsc,
                (ScanState::OscEsc, _) => ScanState::Osc,
            };
        }
    }

    fn apply(&mut self) {
        let params: Vec<u16> = std::str::from_utf8(&self.scan.params)
            .unwrap_or("")
            .split(';')
            .filter_map(|p| p.parse().ok())
            .collect();
        match (self.scan.prefix, params.as_slice()) {
            // CSI > flags u: push onto the stack.
            (Some(b'>'), [flags]) => self.stack.push(*flags),
            // CSI = flags ; mode u: set/union/subtract.
            (Some(b'='), [flags, mode]) => {
                let current = self.stack.pop().unwrap_or(0);
                let next = match mode {
                    2 => current | flags,
                    3 => current & !flags,
                    _ => *flags,
                };
                self.stack.push(next);
            }
            // CSI < number u: pop that many (default one).
            (Some(b'<'), []) => {
                self.stack.pop();
            }
            (Some(b'<'), [count]) => {
                for _ in 0..*count {
                    self.stack.pop();
                }
            }
            _ => {}
        }
    }
}

fn frame(channel: u32, kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(&channel.to_le_bytes());
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn send_frame(stdin: &Mutex<ChildStdin>, channel: u32, kind: u8, payload: &[u8]) -> io::Result<()> {
    let mut writer = stdin
        .lock()
        .map_err(|_| io::Error::other("hc writer poisoned"))?;
    writer.write_all(&frame(channel, kind, payload))?;
    writer.flush()
}

/// One frame, or `None` at a clean end of the stream.
fn read_frame(reader: &mut impl Read) -> io::Result<Option<(u32, u8, Vec<u8>)>> {
    let mut header = [0u8; HEADER];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let channel = u32::from_le_bytes(header[0..4].try_into().expect("4 bytes"));
    let kind = header[4];
    let len = u32::from_le_bytes(header[5..9].try_into().expect("4 bytes")) as usize;
    if len > MAX_PAYLOAD {
        return Err(io::Error::other(format!("hc frame of {len} bytes is too large")));
    }
    let mut payload = vec![0u8; len];
    match reader.read_exact(&mut payload) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    Ok(Some((channel, kind, payload)))
}

/// A framed `hc connect` connection during setup (before the reader thread).
struct Setup {
    stdin: Arc<Mutex<ChildStdin>>,
    reader: BufReader<ChildStdout>,
    next_id: u64,
}

impl Setup {
    fn call(&mut self, method: &str, params: Value) -> io::Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let bytes = serde_json::to_vec(&request).map_err(io::Error::other)?;
        send_frame(&self.stdin, 0, DATA, &bytes)?;
        loop {
            let Some((channel, kind, payload)) = read_frame(&mut self.reader)? else {
                return Err(io::Error::other("hc connect closed during setup"));
            };
            if channel != 0 || kind != DATA {
                continue;
            }
            let reply: Value = serde_json::from_slice(&payload).map_err(io::Error::other)?;
            if reply.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = reply.get("error") {
                let message = error["message"].as_str().unwrap_or("hc request failed");
                let hint = error["data"]["hint"].as_str().unwrap_or("");
                return Err(io::Error::other(if hint.is_empty() {
                    format!("{method}: {message}")
                } else {
                    format!("{method}: {message} ({hint})")
                }));
            }
            return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

fn hc_candidates() -> Vec<PathBuf> {
    if let Ok(bin) = env::var("HC_BIN")
        && !bin.trim().is_empty()
    {
        return vec![PathBuf::from(bin)];
    }
    let mut paths = vec![PathBuf::from("hc")];
    if let Some(home) = env::var_os("HOME") {
        paths.push(PathBuf::from(home).join(".local/bin/hc"));
    }
    paths
}

/// Start `hc connect` and finish the plain-mode `hello` handshake.
fn connect() -> io::Result<(Child, Setup)> {
    let mut last = None;
    for bin in hc_candidates() {
        match Command::new(&bin)
            .arg("connect")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                let mut stdin = child
                    .stdin
                    .take()
                    .ok_or_else(|| io::Error::other("hc connect has no stdin"))?;
                let stdout = child
                    .stdout
                    .take()
                    .ok_or_else(|| io::Error::other("hc connect has no stdout"))?;
                // hello is a plain JSON line; framing begins after its reply.
                let hello = json!({
                    "jsonrpc": "2.0",
                    "id": 0,
                    "method": "hello",
                    "params": { "generation": 1, "framed": true },
                });
                let hello = serde_json::to_string(&hello).map_err(io::Error::other)?;
                writeln!(stdin, "{hello}")?;
                stdin.flush()?;
                let mut reader = BufReader::new(stdout);
                let mut line = String::new();
                if reader.read_line(&mut line)? == 0 {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(io::Error::other("hc connect closed during hello"));
                }
                let reply: Value = serde_json::from_str(line.trim()).map_err(io::Error::other)?;
                if let Some(error) = reply.get("error") {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(io::Error::other(format!(
                        "hc hello: {}",
                        error["message"].as_str().unwrap_or("refused")
                    )));
                }
                if reply["result"]["framed"] != json!(true) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(io::Error::other("hc refused framed mode"));
                }
                let setup = Setup {
                    stdin: Arc::new(Mutex::new(stdin)),
                    reader,
                    next_id: 0,
                };
                return Ok((child, setup));
            }
            Err(error) => last = Some(error),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("hc binary not found")))
}

fn is_live(terminal: &Value) -> bool {
    terminal["state"].as_str() == Some("live")
}

fn labels_match(terminal: &Value, labels: &Value) -> bool {
    labels.as_object().is_some_and(|wanted| {
        wanted.iter().all(|(key, value)| {
            terminal["labels"].get(key).and_then(Value::as_str) == value.as_str()
        })
    })
}

fn address_of(terminal: &Value) -> Option<String> {
    terminal["address"].as_str().map(str::to_string)
}

fn run_terminal(
    setup: &mut Setup,
    argv: Vec<String>,
    cwd: &Path,
    name: &str,
    rows: u16,
    cols: u16,
    labels: &Value,
) -> io::Result<String> {
    let result = setup.call(
        "terminal.run",
        json!({
            "argv": argv,
            "cwd": cwd.to_string_lossy(),
            "name": name,
            "no_parent": true,
            "size": { "cols": cols, "rows": rows },
            "labels": labels,
        }),
    )?;
    address_of(&result)
        .ok_or_else(|| io::Error::other("hc run returned no terminal address"))
}

fn attach(
    setup: &mut Setup,
    address: &str,
    rows: u16,
    cols: u16,
) -> io::Result<u32> {
    let result = setup.call(
        "terminal.attach",
        json!({
            "terminal": address,
            "cols": cols,
            "rows": rows,
            "history": 0,
        }),
    )?;
    result["channel"]
        .as_u64()
        .map(|channel| channel as u32)
        .ok_or_else(|| io::Error::other("hc attach returned no channel"))
}

/// Codex's startup update prompt blocks the embedded chat until someone
/// answers it. Updates are managed by the owner, so disable the startup check
/// for this invocation instead of auto-clicking a version-dependent popup.
fn codex_argv(context: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        "codex".to_string(),
        "--dangerously-bypass-approvals-and-sandbox".to_string(),
        "-c".to_string(),
        "check_for_update_on_startup=false".to_string(),
    ];
    if let Some(context) = context {
        argv.push(context.to_string());
    }
    argv
}

/// FNV-1a of the working directory, so two kbots TUIs in different projects
/// never share a "free" Codex terminal.
fn scope(cwd: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in cwd.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", (hash ^ (hash >> 32)) as u32)
}

fn slug(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for c in input.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out.truncate(9);
    while out.ends_with('-') {
        out.pop();
    }
    out
}

fn terminal_name(binding: &str, cwd: &Path, taken: &[String]) -> String {
    let scope = scope(cwd);
    let slug = slug(binding);
    let base = if slug.is_empty() {
        format!("kbots-codex-{scope}")
    } else {
        format!("kbots-codex-{slug}-{scope}")
    };
    if !taken.iter().any(|name| name == &base) {
        return base;
    }
    for n in 2..=99 {
        let candidate = format!("{base}-{n}");
        if !taken.iter().any(|name| name == &candidate) {
            return candidate;
        }
    }
    base
}

/// One attached viewer of a herdr-core terminal.
pub struct CodexTerm {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    parser: Arc<Mutex<vt100::Parser>>,
    kitty: Arc<Mutex<Kitty>>,
    state: Arc<Mutex<ViewerState>>,
    channel: u32,
    rows: u16,
    cols: u16,
    address: String,
}

impl CodexTerm {
    /// Find the live `hc` terminal for this binding (or start Codex in a new
    /// one), attach to it, and start painting its output into `parser`.
    pub fn spawn(
        binding: &str,
        rows: u16,
        cols: u16,
        cwd: &Path,
        context: Option<&str>,
    ) -> io::Result<Self> {
        let rows = rows.max(1);
        let cols = cols.max(1);
        let (mut child, mut setup) = connect()?;
        let labels = json!({
            "kbots.codex": "1",
            "kbots.scope": scope(cwd),
            "kbots.task": binding,
            // Old terminals (before the startup update check was disabled)
            // must not be reused: they may be parked on the update popup.
            "kbots.update_check": "off",
        });

        let list = match setup.call("terminal.list", json!({})) {
            Ok(list) => list,
            Err(error) => {
                let _ = child.kill();
                return Err(error);
            }
        };
        let terminals = list["terminals"].as_array().cloned().unwrap_or_default();
        let live: Vec<Value> = terminals.iter().filter(|t| is_live(t)).cloned().collect();
        let existing = live
            .iter()
            .find(|terminal| labels_match(terminal, &labels))
            .and_then(address_of);

        let argv = codex_argv(context);
        let names: Vec<String> = live
            .iter()
            .filter_map(|terminal| terminal["name"].as_str().map(str::to_string))
            .collect();
        let name = terminal_name(binding, cwd, &names);

        let start = |setup: &mut Setup| -> io::Result<String> {
            run_terminal(setup, argv.clone(), cwd, &name, rows, cols, &labels)
        };
        let address = match existing {
            Some(address) => address,
            None => match start(&mut setup) {
                Ok(address) => address,
                Err(error) => {
                    let _ = child.kill();
                    return Err(error);
                }
            },
        };

        let channel = match attach(&mut setup, &address, rows, cols) {
            Ok(channel) => channel,
            Err(error) if error.to_string().contains("finished") => {
                // It exited between the list and the attach: start a fresh one.
                let address = match start(&mut setup) {
                    Ok(address) => address,
                    Err(error) => {
                        let _ = child.kill();
                        return Err(error);
                    }
                };
                match attach(&mut setup, &address, rows, cols) {
                    Ok(channel) => channel,
                    Err(error) => {
                        let _ = child.kill();
                        return Err(error);
                    }
                }
            }
            Err(error) => {
                let _ = child.kill();
                return Err(error);
            }
        };

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK_LINES)));
        let kitty = Arc::new(Mutex::new(Kitty::default()));
        let state = Arc::new(Mutex::new(ViewerState::default()));
        let stdin = setup.stdin;
        let Setup { reader, .. } = setup;
        let attach_channel = channel;
        let thread_parser = Arc::clone(&parser);
        let thread_kitty = Arc::clone(&kitty);
        let thread_state = Arc::clone(&state);
        let thread_stdin = Arc::clone(&stdin);
        thread::spawn(move || {
            let mut reader = reader;
            let mut reason = "hc connection closed".to_string();
            loop {
                let frame = match read_frame(&mut reader) {
                    Ok(Some(frame)) => frame,
                    Ok(None) => break,
                    Err(error) => {
                        reason = format!("hc connection: {error}");
                        break;
                    }
                };
                let (channel, kind, payload) = frame;
                if channel != attach_channel {
                    continue;
                }
                match kind {
                    DATA => {
                        if let Ok(mut kitty) = thread_kitty.lock() {
                            kitty.scan(&payload);
                        }
                        if let Ok(mut parser) = thread_parser.lock() {
                            parser.process(&payload);
                        }
                        let credit = (payload.len() as u32).to_le_bytes();
                        if send_frame(&thread_stdin, channel, CREDIT, &credit).is_err() {
                            break;
                        }
                    }
                    CLOSE => {
                        reason = close_reason(&payload);
                        break;
                    }
                    _ => {}
                }
            }
            if let Ok(mut state) = thread_state.lock()
                && state.exit.is_none()
            {
                state.exit = Some(reason);
            }
        });

        Ok(Self {
            child,
            stdin,
            parser,
            kitty,
            state,
            channel,
            rows,
            cols,
            address,
        })
    }

    /// The herdr-core address (`t12y`) this viewer is attached to.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Keep the attached terminal and the parser in sync with the drawn area.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        if let Ok(mut parser) = self.parser.lock() {
            parser.screen_mut().set_size(rows, cols);
        }
        let mut size = Vec::with_capacity(4);
        size.extend_from_slice(&cols.to_le_bytes());
        size.extend_from_slice(&rows.to_le_bytes());
        let _ = send_frame(&self.stdin, self.channel, RESIZE, &size);
    }

    pub fn write_input(&self, bytes: &[u8]) -> io::Result<()> {
        send_frame(&self.stdin, self.channel, DATA, bytes)
    }

    /// Scrollback offset in rows from the top of the screen; 0 = live view.
    pub fn scroll_offset(&self) -> usize {
        self.parser
            .lock()
            .map(|parser| parser.screen().scrollback())
            .unwrap_or(0)
    }

    pub fn scroll_len(&self) -> usize {
        SCROLLBACK_LINES
    }

    pub fn set_scroll(&self, offset: usize) {
        if let Ok(mut parser) = self.parser.lock() {
            parser.screen_mut().set_scrollback(offset);
        }
    }

    /// Reap the viewer if the program ended; returns true when it is gone.
    pub fn has_exited(&mut self) -> bool {
        self.state
            .lock()
            .map(|state| state.exit.is_some())
            .unwrap_or(true)
    }

    /// Where the wheel goes: the program when it asked for mouse reports (as
    /// herdr-core does), otherwise the local scrollback. Returns true when it
    /// was forwarded.
    pub fn wheel(&self, up: bool, column: u16, row: u16) -> bool {
        let (mode, encoding) = match self.parser.lock() {
            Ok(parser) => (
                parser.screen().mouse_protocol_mode(),
                parser.screen().mouse_protocol_encoding(),
            ),
            Err(_) => return false,
        };
        if matches!(mode, vt100::MouseProtocolMode::None) {
            return false;
        }
        let button = if up { 64 } else { 65 };
        let bytes = encode_mouse(button, column, row, encoding, self.cols, self.rows, true);
        self.write_input(&bytes).is_ok()
    }

    /// Forward a press/release to the program when mouse reporting is on.
    pub fn mouse(&self, kind: MouseEventKind, column: u16, row: u16) -> bool {
        let (mode, encoding) = match self.parser.lock() {
            Ok(parser) => (
                parser.screen().mouse_protocol_mode(),
                parser.screen().mouse_protocol_encoding(),
            ),
            Err(_) => return false,
        };
        if matches!(mode, vt100::MouseProtocolMode::None) {
            return false;
        }
        let (button, pressed) = match kind {
            MouseEventKind::Down(button) => (mouse_button(button), true),
            MouseEventKind::Up(button) => (mouse_button(button), false),
            MouseEventKind::Drag(button) => (mouse_button(button) + 32, true),
            _ => return false,
        };
        let bytes = encode_mouse(button, column, row, encoding, self.cols, self.rows, pressed);
        self.write_input(&bytes).is_ok()
    }

    /// Encode a key the way the attached program asked for: kitty keyboard
    /// protocol flags tracked from its output, plus application cursor keys.
    pub fn key_bytes(&self, key: &KeyEvent) -> Option<Vec<u8>> {
        let (app_cursor, kitty) = {
            let kitty = self.kitty.lock().map(|kitty| kitty.flags()).unwrap_or(0);
            let app_cursor = self
                .parser
                .lock()
                .map(|parser| parser.screen().application_cursor())
                .unwrap_or(false);
            (app_cursor, kitty)
        };
        encode_key(key, app_cursor, kitty)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, title: &str, focused: bool) {
        let border = if focused {
            ratatui::style::Color::LightGreen
        } else {
            ratatui::style::Color::DarkGray
        };
        let block = Block::default()
            .title(format!(" {title} "))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(ratatui::style::Style::default().fg(border));
        let scrolled = self.scroll_offset();
        let length = self.scroll_len();
        let block = if scrolled > 0 {
            block.title_bottom(format!(
                " scrollback: {scrolled}/{length} rows · wheel or Ctrl+Shift+Down to return to live "
            ))
        } else {
            block
        };
        let parser = match self.parser.lock() {
            Ok(parser) => parser,
            Err(_) => {
                frame.render_widget(
                    ratatui::widgets::Paragraph::new("codex terminal state unavailable"),
                    area,
                );
                return;
            }
        };
        let pseudo = PseudoTerminal::new(parser.screen()).block(block);
        frame.render_widget(pseudo, area);
    }
}

fn encode_key(key: &KeyEvent, app_cursor: bool, kitty: u16) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let scroll = ctrl
            && shift
            && matches!(key.code, KeyCode::Up | KeyCode::Down);
        if scroll {
            return None;
        }
        if ctrl && matches!(key.code, KeyCode::Char(']') | KeyCode::Char('5')) {
            return None; // host chord: Ctrl+]
        }
        let event = match key.kind {
            KeyEventKind::Press => 1,
            KeyEventKind::Repeat => 2,
            KeyEventKind::Release => 3,
        };
        let mods = 1 + u16::from(shift) + (u16::from(alt) << 1) + (u16::from(ctrl) << 2);
        let report_events = kitty & 2 != 0;
        let all_keys = kitty & 8 != 0;
        let disambiguate = kitty & 1 != 0;
        let kitty_seq = |code: u32, mods: u16| kitty_sequence(code, mods, event, report_events);

        if all_keys {
            if let Some(code) = kitty_code(&key.code) {
                return Some(kitty_seq(code, mods));
            }
        }
        match key.code {
            KeyCode::Char(c) => {
                if all_keys {
                    return Some(kitty_seq(c as u32, mods));
                }
                if disambiguate && (ctrl || alt) {
                    let code = if ctrl { c.to_ascii_lowercase() } else { c };
                    return Some(kitty_seq(code as u32, mods));
                }
                if ctrl {
                    let byte = match c.to_ascii_lowercase() {
                        'a'..='z' => Some((c.to_ascii_lowercase() as u8) - b'a' + 1),
                        '[' | '3' => Some(0x1b),
                        '\\' => Some(0x1c),
                        ']' => Some(0x1d),
                        '^' => Some(0x1e),
                        '_' | '-' => Some(0x1f),
                        '@' | ' ' => Some(0x00),
                        _ => None,
                    };
                    byte.map(|b| vec![b])
                } else if alt {
                    let mut bytes = vec![0x1b];
                    let mut buffer = [0_u8; 4];
                    bytes.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
                    Some(bytes)
                } else {
                    let mut buffer = [0_u8; 4];
                    Some(c.encode_utf8(&mut buffer).as_bytes().to_vec())
                }
            }
            KeyCode::Enter => {
                if disambiguate || all_keys {
                    Some(kitty_seq(13, mods))
                } else {
                    Some(b"\r".to_vec())
                }
            }
            KeyCode::Backspace => {
                if disambiguate || all_keys {
                    Some(kitty_seq(127, mods))
                } else {
                    Some(vec![0x7f])
                }
            }
            KeyCode::Tab => {
                if disambiguate || all_keys {
                    Some(kitty_seq(9, mods))
                } else {
                    Some(vec![0x09])
                }
            }
            KeyCode::BackTab => {
                if disambiguate || all_keys {
                    Some(kitty_seq(9, mods | 2))
                } else {
                    Some(b"\x1b[Z".to_vec())
                }
            }
            KeyCode::Esc => {
                if disambiguate || all_keys {
                    Some(kitty_seq(27, mods))
                } else {
                    Some(vec![0x1b])
                }
            }
            KeyCode::Up => Some(cursor_key(b'A', app_cursor, mods)),
            KeyCode::Down => Some(cursor_key(b'B', app_cursor, mods)),
            KeyCode::Right => Some(cursor_key(b'C', app_cursor, mods)),
            KeyCode::Left => Some(cursor_key(b'D', app_cursor, mods)),
            KeyCode::Home => Some(cursor_key(b'H', app_cursor, mods)),
            KeyCode::End => Some(cursor_key(b'F', app_cursor, mods)),
            KeyCode::Insert => Some(tilde_key(2, mods)),
            KeyCode::Delete => Some(tilde_key(3, mods)),
            KeyCode::PageUp => Some(tilde_key(5, mods)),
            KeyCode::PageDown => Some(tilde_key(6, mods)),
            KeyCode::F(number @ 1..=4) => Some(ss3_key(b'P' + number - 1, mods)),
            KeyCode::F(number @ 5..=12) => {
                let suffix = ["15", "17", "18", "19", "20", "21", "23", "24"];
                Some(tilde_key_str(suffix[(number - 5) as usize], mods))
            }
            _ => None,
        }
}

impl Drop for CodexTerm {
    fn drop(&mut self) {
        // Detach; the program keeps running in herdr-core.
        let _ = send_frame(&self.stdin, self.channel, CLOSE, &[]);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn close_reason(payload: &[u8]) -> String {
    let Ok(reason) = serde_json::from_slice::<Value>(payload) else {
        return "exited".to_string();
    };
    let kind = reason["reason"].as_str().unwrap_or("exited");
    match reason["exit_code"].as_i64() {
        Some(code) => format!("{kind} (exit {code})"),
        None => kind.to_string(),
    }
}

fn kitty_sequence(code: u32, mods: u16, event: u8, report_events: bool) -> Vec<u8> {
    let params = if mods <= 1 && (!report_events || event == 1) {
        code.to_string()
    } else if report_events && event != 1 {
        format!("{code};{mods}:{event}")
    } else {
        format!("{code};{mods}")
    };
    format!("\x1b[{params}u").into_bytes()
}

fn kitty_code(code: &KeyCode) -> Option<u32> {
    Some(match code {
        KeyCode::Esc => 27,
        KeyCode::Enter => 13,
        KeyCode::Tab => 9,
        KeyCode::Backspace => 127,
        KeyCode::Insert => 57_348,
        KeyCode::Delete => 57_349,
        KeyCode::Left => 57_350,
        KeyCode::Right => 57_351,
        KeyCode::Up => 57_352,
        KeyCode::Down => 57_353,
        KeyCode::PageUp => 57_354,
        KeyCode::PageDown => 57_355,
        KeyCode::Home => 57_356,
        KeyCode::End => 57_357,
        KeyCode::CapsLock => 57_358,
        KeyCode::ScrollLock => 57_359,
        KeyCode::NumLock => 57_360,
        KeyCode::PrintScreen => 57_361,
        KeyCode::Pause => 57_362,
        KeyCode::Menu => 57_363,
        KeyCode::F(number @ 1..=12) => 57_364 + u32::from(*number) - 1,
        KeyCode::BackTab => 9,
        _ => return None,
    })
}

fn cursor_key(final_byte: u8, app_cursor: bool, mods: u16) -> Vec<u8> {
    if mods <= 1 {
        if app_cursor {
            return vec![0x1b, b'O', final_byte];
        }
        return format!("\x1b[{}", final_byte as char).into_bytes();
    }
    format!("\x1b[1;{mods}{}", final_byte as char).into_bytes()
}

fn ss3_key(final_byte: u8, mods: u16) -> Vec<u8> {
    if mods <= 1 {
        return vec![0x1b, b'O', final_byte];
    }
    format!("\x1b[1;{mods}{}", final_byte as char).into_bytes()
}

fn tilde_key(number: u8, mods: u16) -> Vec<u8> {
    tilde_key_str(&number.to_string(), mods)
}

fn tilde_key_str(number: &str, mods: u16) -> Vec<u8> {
    if mods <= 1 {
        return format!("\x1b[{number}~").into_bytes();
    }
    format!("\x1b[{number};{mods}~").into_bytes()
}

fn mouse_button(button: MouseButton) -> u16 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

fn encode_mouse(
    button: u16,
    column: u16,
    row: u16,
    encoding: vt100::MouseProtocolEncoding,
    cols: u16,
    rows: u16,
    pressed: bool,
) -> Vec<u8> {
    let x = column.clamp(1, cols.max(1));
    let y = row.clamp(1, rows.max(1));
    match encoding {
        vt100::MouseProtocolEncoding::Sgr => {
            let final_byte = if pressed { 'M' } else { 'm' };
            format!("\x1b[<{button};{x};{y}{final_byte}").into_bytes()
        }
        vt100::MouseProtocolEncoding::Utf8 => {
            let mut out = b"\x1b[M".to_vec();
            for value in [32 + button, 32 + x, 32 + y] {
                let c = char::from_u32(u32::from(value)).unwrap_or('\u{fffd}');
                let mut buffer = [0_u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
            }
            out
        }
        vt100::MouseProtocolEncoding::Default => {
            let mut out = b"\x1b[M".to_vec();
            if pressed {
                out.extend_from_slice(&[
                    (32 + button).min(255) as u8,
                    (32 + x).min(255) as u8,
                    (32 + y).min(255) as u8,
                ]);
            } else {
                out.extend_from_slice(&[3 + 32, (32 + x).min(255) as u8, (32 + y).min(255) as u8]);
            }
            out
        }
    }
}

/// One viewer per binding. Switching the binding detaches the old viewer
/// (the program stays alive in herdr-core) and attaches the new one.
#[derive(Default)]
pub struct CodexTerminals {
    terms: HashMap<String, CodexTerm>,
    pub order: Vec<String>,
}

impl CodexTerminals {
    pub fn get(&self, key: &str) -> Option<&CodexTerm> {
        self.terms.get(key)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut CodexTerm> {
        self.terms.get_mut(key)
    }

    pub fn insert(&mut self, key: String, term: CodexTerm) {
        if !self.order.iter().any(|existing| existing == &key) {
            self.order.push(key.clone());
        }
        self.terms.insert(key, term);
    }

    pub fn remove(&mut self, key: &str) {
        self.terms.remove(key);
        self.order.retain(|existing| existing != key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn frames_round_trip_and_end_cleanly() {
        let bytes = frame(3, DATA, b"hello");
        let mut reader = Cursor::new(bytes);
        let (channel, kind, payload) = read_frame(&mut reader).unwrap().unwrap();
        assert_eq!((channel, kind, payload.as_slice()), (3, DATA, b"hello".as_slice()));
        assert!(read_frame(&mut reader).unwrap().is_none());
    }

    #[test]
    fn kitty_flags_follow_push_set_and_pop() {
        let mut kitty = Kitty::default();
        kitty.scan(b"\x1b[>7u");
        assert_eq!(kitty.flags(), 7);
        // A sequence split across reads is still understood.
        kitty.scan(b"\x1b[=");
        kitty.scan(b"8;2u");
        assert_eq!(kitty.flags(), 15);
        kitty.scan(b"\x1b[>7u");
        assert_eq!(kitty.flags(), 7);
        kitty.scan(b"\x1b[<1u");
        assert_eq!(kitty.flags(), 15);
        // Noise that is not a keyboard mode leaves the flags alone.
        kitty.scan(b"\x1b[?25l\x1b]0;title\x07");
        assert_eq!(kitty.flags(), 15);
    }

    #[test]
    fn keys_use_the_modes_the_program_set() {
        // No kitty, no application cursor: classic ANSI.
        assert_eq!(encode_key(&key(KeyCode::Up, KeyModifiers::NONE), false, 0), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode_key(&key(KeyCode::Up, KeyModifiers::NONE), true, 0), Some(b"\x1bOA".to_vec()));
        assert_eq!(encode_key(&key(KeyCode::Char('c'), KeyModifiers::CONTROL), false, 0), Some(vec![0x03]));
        // Codex pushes kitty flags 7 (disambiguate, events, alternate keys).
        assert_eq!(encode_key(&key(KeyCode::Enter, KeyModifiers::SHIFT), false, 7), Some(b"\x1b[13;2u".to_vec()));
        assert_eq!(encode_key(&key(KeyCode::Enter, KeyModifiers::NONE), false, 7), Some(b"\x1b[13u".to_vec()));
        assert_eq!(encode_key(&key(KeyCode::Char('c'), KeyModifiers::CONTROL), false, 7), Some(b"\x1b[99;5u".to_vec()));
        assert_eq!(encode_key(&key(KeyCode::Char('a'), KeyModifiers::NONE), false, 7), Some(b"a".to_vec()));
        assert_eq!(encode_key(&key(KeyCode::Esc, KeyModifiers::NONE), false, 7), Some(b"\x1b[27u".to_vec()));
        // The host chord never reaches the program.
        assert_eq!(encode_key(&key(KeyCode::Char(']'), KeyModifiers::CONTROL), false, 7), None);
    }

    #[test]
    fn wheel_uses_the_encoding_the_program_asked_for() {
        assert_eq!(
            encode_mouse(64, 10, 5, vt100::MouseProtocolEncoding::Sgr, 80, 24, true),
            b"\x1b[<64;10;5M".to_vec()
        );
        // Coordinates are clamped to the terminal.
        assert_eq!(
            encode_mouse(65, 0, 99, vt100::MouseProtocolEncoding::Sgr, 80, 24, true),
            b"\x1b[<65;1;24M".to_vec()
        );
    }

    #[test]
    fn terminal_names_are_short_and_unique() {
        let cwd = Path::new("/tmp/opencode/kbots");
        assert_eq!(slug("TASK-12"), "task-12");
        assert_eq!(slug(""), "");
        let first = terminal_name("TASK-12", cwd, &[]);
        assert!(first.starts_with("kbots-codex-task-12-"));
        assert!(first.len() <= 32);
        let second = terminal_name("TASK-12", cwd, &[first.clone()]);
        assert_ne!(first, second);
        assert!(second.len() <= 32);
    }

    #[test]
    fn codex_disables_the_startup_update_prompt() {
        let argv = codex_argv(None);
        assert_eq!(
            argv,
            vec![
                "codex",
                "--dangerously-bypass-approvals-and-sandbox",
                "-c",
                "check_for_update_on_startup=false",
            ]
        );
        assert_eq!(codex_argv(Some("Continue TASK-12")), [argv, vec!["Continue TASK-12".to_string()]].concat());
    }
}
