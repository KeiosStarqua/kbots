//! Embedded interactive Codex terminal.
//!
//! Section 1 of the TUI is a real `codex` CLI session running in a
//! pseudo-terminal (portable-pty) and rendered with `tui_term`. kbots forwards
//! key presses to Codex and renders what Codex prints; kbots itself never
//! interprets Codex output. When the Codex page is focused, every key except
//! the escape chord `Ctrl+]` is forwarded to the process; `Ctrl+Shift+Up` /
//! `Ctrl+Shift+Down` (or the mouse wheel) scroll the pty scrollback.
//!
//! One terminal per bound task lives in memory (`CodexTerminals`): switching
//! the binding with `[`/`]` parks the current terminal and resumes it when
//! you cycle back, so a task's Codex session keeps its context. `codex`
//! processes are children of the TUI; quitting kbots kills them.

use std::{
    collections::HashMap,
    io::{self, Read, Write},
    path::Path,
    sync::{Arc, Mutex},
    thread,
};

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders, BorderType},
};
use tui_term::{
    vt100,
    widget::PseudoTerminal,
};

/// Number of scrollback lines kept per embedded terminal.
const SCROLLBACK_LINES: usize = 1_000;

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

pub struct CodexTerm {
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Mutex<Box<dyn Write + Send>>,
    rows: u16,
    cols: u16,
}

impl CodexTerm {
    /// Spawn `codex` in `cwd`, optionally with a first message bound to a task.
    pub fn spawn(rows: u16, cols: u16, cwd: &Path, context: Option<&str>) -> io::Result<Self> {
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(portable_pty::PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(io::Error::other)?;
        let mut command = portable_pty::CommandBuilder::new("codex");
        command.arg("--sandbox");
        command.arg("workspace-write");
        command.arg("--ask-for-approval");
        command.arg("on-request");
        if let Some(context) = context {
            command.arg(context);
        }
        command.cwd(cwd);
        let child = pair.slave.spawn_command(command).map_err(io::Error::other)?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(io::Error::other)?;
        let writer = pair.master.take_writer().map_err(io::Error::other)?;
        let parser = Arc::new(Mutex::new(vt100::Parser::new(
            rows.max(1),
            cols.max(1),
            SCROLLBACK_LINES,
        )));
        let parser_reader = Arc::clone(&parser);
        thread::spawn(move || {
            let mut buffer = [0_u8; 16 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        if let Ok(mut parser) = parser_reader.lock() {
                            parser.process(&buffer[..read]);
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            master: pair.master,
            child,
            parser,
            writer: Mutex::new(writer),
            rows: rows.max(1),
            cols: cols.max(1),
        })
    }

    /// Keep the pty and the parser in sync with the drawn area.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        let _ = self.master.resize(portable_pty::PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        if let Ok(mut parser) = self.parser.lock() {
            let _ = parser.screen_mut().set_size(rows, cols);
        }
    }

    pub fn write_input(&self, bytes: &[u8]) -> io::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("codex writer poisoned"))?;
        writer.write_all(bytes)
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

    /// Reap the child if it exited; returns true when the process is gone.
    pub fn has_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
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
            block.title_bottom(format!(" scrollback: {scrolled}/{length} rows · wheel or Ctrl+Shift+Down to return to live "))
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

impl Drop for CodexTerm {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One live terminal per binding; switching keeps both alive.
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

/// Map a crossterm key event to bytes for the pty. Returns None for keys the
/// host should consume (navigation chord, scroll chord).
pub fn key_to_pty_bytes(key: &KeyEvent) -> Option<Vec<u8>> {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let scroll_up = control && shift && matches!(key.code, KeyCode::Up);
    let scroll_down = control && shift && matches!(key.code, KeyCode::Down);
    if scroll_up || scroll_down {
        return None; // handled by the scrollback logic
    }
    match key.code {
        KeyCode::Char(']') if control => None, // host chord: Ctrl+]
        KeyCode::Char(c) => {
            if control {
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
        KeyCode::Enter => Some(b"\r".to_vec()),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Tab => Some(vec![0x09]),
        KeyCode::BackTab => Some(b"\x1b[Z".to_vec()),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
        KeyCode::F(number @ 1..=4) => Some(vec![0x1b, b'O', b'P' + number - 1]),
        KeyCode::F(number @ 5..=12) => {
            let suffix = ["15~", "17~", "18~", "19~", "20~", "21~", "23~", "24~"];
            Some(format!("\x1b[{}", suffix[(number - 5) as usize]).into_bytes())
        }
        _ => None,
    }
}
