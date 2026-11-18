mod rpc;

use std::collections::BTreeMap;
use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseEventKind,
    },
    execute,
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Row, Table, Wrap},
};

use rpc::{
    AgentsList, Approval, EventItem, InboxItem, RpcClient, RpcError, SystemStatus, Task,
    brief_entry_line, truncate_chars,
};

const NAVIGATION: [&str; 6] = [
    "Overview",
    "Projects",
    "Tasks",
    "Agents",
    "Approvals",
    "Activity log",
];

const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const INBOX_KIND_ORDER: [&str; 5] = ["approval", "blocked", "failed", "queued", "review"];

// ---------------------------------------------------------------------------
// Background fetch plumbing
// ---------------------------------------------------------------------------

struct Snapshot {
    tasks: Result<Vec<Task>, RpcError>,
    agents: Result<AgentsList, RpcError>,
    approvals: Result<Vec<Approval>, RpcError>,
    events: Result<Vec<EventItem>, RpcError>,
    status: Result<SystemStatus, RpcError>,
    inbox: Result<Vec<InboxItem>, RpcError>,
}

enum BgResult {
    Snapshot(Box<Snapshot>),
    Brief(Result<serde_json::Value, RpcError>),
    Inbox(Result<Vec<InboxItem>, RpcError>),
    TaskCreated(Result<rpc::TaskCreateResult, RpcError>),
    TaskMessaged(Result<rpc::TaskMessageResult, RpcError>),
    TaskUpdated(Result<rpc::TaskUpdateResult, RpcError>),
}

fn fetch_snapshot(client: &RpcClient) -> Snapshot {
    Snapshot {
        tasks: client.tasks_list(),
        agents: client.agents_list(),
        approvals: client.approvals_list_pending(),
        events: client.events_list(50),
        status: client.system_status(),
        inbox: client.tasks_inbox(),
    }
}

// ---------------------------------------------------------------------------
// UI state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Connection {
    Connecting,
    Online,
    Disconnected,
}

#[derive(Default)]
struct SectionErrors {
    overview: Option<String>,
    projects: Option<String>,
    tasks: Option<String>,
    agents: Option<String>,
    approvals: Option<String>,
    activity: Option<String>,
}

struct BriefView {
    generated_at: String,
    counts: Vec<(String, String)>,
    total_label: Option<String>,
    needs_action: Vec<String>,
    running: Vec<String>,
    done: Vec<String>,
}

enum OverlayState<T> {
    Loading,
    Ready(T),
    Failed(String),
}

enum Overlay {
    Brief { state: OverlayState<BriefView> },
    Inbox { state: OverlayState<Vec<InboxItem>> },
}

impl Overlay {
    fn title(&self) -> &'static str {
        match self {
            Overlay::Brief { .. } => " /brief ",
            Overlay::Inbox { .. } => " /inbox ",
        }
    }
}

struct App {
    // navigation (kept from the original TUI)
    selected: usize,
    prefix_pending: bool,
    navigator_preview: Option<usize>,
    focused_panel: usize,
    zoomed: bool,

    // daemon state
    connection: Connection,
    tasks: Vec<Task>,
    agents: AgentsList,
    approvals: Vec<Approval>,
    events: Vec<EventItem>,
    system: Option<SystemStatus>,
    inbox_items: Vec<InboxItem>,
    inbox_supported: bool,
    errors: SectionErrors,
    refreshing: bool,
    loaded: bool,
    last_refresh_ok: Option<Instant>,
    refresh_started_at: Option<Instant>,
    next_refresh: Instant,
    bg_tx: Sender<BgResult>,
    bg_rx: Receiver<BgResult>,

    // composer + overlays
    composer_focused: bool,
    composer_text: String,
    composer_note: Option<(String, bool)>, // (text, is_error)
    overlay: Option<Overlay>,
    overlay_scroll: usize,
}

impl App {
    fn new() -> Self {
        let (bg_tx, bg_rx) = mpsc::channel();
        Self {
            selected: 0,
            prefix_pending: false,
            navigator_preview: None,
            focused_panel: 0,
            zoomed: false,

            connection: Connection::Connecting,
            tasks: Vec::new(),
            agents: AgentsList::default(),
            approvals: Vec::new(),
            events: Vec::new(),
            system: None,
            inbox_items: Vec::new(),
            inbox_supported: true,
            errors: SectionErrors::default(),
            refreshing: false,
            loaded: false,
            last_refresh_ok: None,
            refresh_started_at: None,
            next_refresh: Instant::now(),
            bg_tx,
            bg_rx,

            composer_focused: false,
            composer_text: String::new(),
            composer_note: None,
            overlay: None,
            overlay_scroll: 0,
        }
    }

    fn active_section(&self) -> usize {
        self.navigator_preview.unwrap_or(self.selected)
    }

    fn move_section(&mut self, offset: isize) {
        let current = self.active_section();
        let len = NAVIGATION.len() as isize;
        let next = (current as isize + offset).rem_euclid(len) as usize;
        if self.navigator_preview.is_some() {
            self.navigator_preview = Some(next);
        } else {
            self.selected = next;
        }
    }

    fn move_panel(&mut self, offset: isize) {
        const PANELS: usize = 4;
        self.focused_panel =
            (self.focused_panel as isize + offset).rem_euclid(PANELS as isize) as usize;
    }

    // -- background work ----------------------------------------------------

    fn request_refresh(&mut self) {
        if self.refreshing {
            return;
        }
        self.next_refresh = Instant::now();
    }

    fn start_refresh(&mut self) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        self.refresh_started_at = Some(Instant::now());
        let tx = self.bg_tx.clone();
        thread::spawn(move || {
            let client = RpcClient::new();
            let _ = tx.send(BgResult::Snapshot(Box::new(fetch_snapshot(&client))));
        });
    }

    fn spawn_bg(&mut self, work: impl FnOnce(RpcClient) -> BgResult + Send + 'static) {
        let tx = self.bg_tx.clone();
        thread::spawn(move || {
            let client = RpcClient::new();
            let _ = tx.send(work(client));
        });
    }

    fn refresh_due(&self) -> bool {
        !self.refreshing && Instant::now() >= self.next_refresh
    }

    /// Drain every completed background job. Returns true when any state
    /// was applied and the UI should redraw.
    fn apply_pending(&mut self) -> bool {
        let mut changed = false;
        loop {
            match self.bg_rx.try_recv() {
                Ok(result) => {
                    if let BgResult::Snapshot(_) = result {
                        self.refreshing = false;
                    }
                    self.apply_bg(result);
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.refreshing = false;
                    break;
                }
            }
        }
        changed
    }

    fn apply_bg(&mut self, result: BgResult) {
        match result {
            BgResult::Snapshot(snapshot) => self.apply_snapshot(*snapshot),
            BgResult::Brief(result) => {
                let state = match result {
                    Ok(value) => OverlayState::Ready(parse_brief(&value)),
                    Err(error) => OverlayState::Failed(brief_error(&error)),
                };
                self.overlay = Some(Overlay::Brief { state });
                self.overlay_scroll = 0;
            }
            BgResult::Inbox(result) => {
                let state = match result {
                    Ok(items) => {
                        self.inbox_items = items.clone();
                        self.inbox_supported = true;
                        OverlayState::Ready(items)
                    }
                    Err(error) => OverlayState::Failed(brief_error(&error)),
                };
                self.overlay = Some(Overlay::Inbox { state });
                self.overlay_scroll = 0;
            }
            BgResult::TaskCreated(result) => {
                let note = match result {
                    Ok(created) => {
                        let suffix = if created.status.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", created.status)
                        };
                        let dispatch = created
                            .ceo_agent_id
                            .as_deref()
                            .map(|id| format!(" → ceo {}", truncate_chars(id, 8)))
                            .unwrap_or_default();
                        (format!("created {}{}{}", created.task_id, suffix, dispatch), false)
                    }
                    Err(error) => (error_note(&error), true),
                };
                self.composer_note = Some(note);
                self.request_refresh();
            }
            BgResult::TaskMessaged(result) => {
                let note = match result {
                    Ok(sent) => {
                        let via = sent
                            .message_id
                            .as_deref()
                            .map(|id| format!(" (msg {})", truncate_chars(id, 10)))
                            .unwrap_or_default();
                        (
                            format!("task {}: message {}{}", sent.task_id, sent.outcome, via),
                            false,
                        )
                    }
                    Err(error) => (error_note(&error), true),
                };
                self.composer_note = Some(note);
                self.request_refresh();
            }
            BgResult::TaskUpdated(result) => {
                let note = match result {
                    Ok(updated) => (
                        format!("task {} → {}", updated.task_id, updated.status),
                        false,
                    ),
                    Err(error) => (error_note(&error), true),
                };
                self.composer_note = Some(note);
                self.request_refresh();
            }
        }
    }

    fn apply_snapshot(&mut self, snapshot: Snapshot) {
        let unreachable = [
            snapshot.tasks.as_ref().err(),
            snapshot.agents.as_ref().err(),
            snapshot.approvals.as_ref().err(),
            snapshot.events.as_ref().err(),
            snapshot.status.as_ref().err(),
            snapshot.inbox.as_ref().err(),
        ]
        .into_iter()
        .flatten()
        .filter(|error| error.is_unreachable())
        .count();

        match snapshot.tasks {
            Ok(tasks) => {
                self.tasks = tasks;
                self.errors.tasks = None;
            }
            Err(error) => self.errors.tasks = Some(error.detail()),
        }
        match snapshot.agents {
            Ok(agents) => {
                self.agents = agents;
                self.errors.agents = None;
                self.errors.projects = None;
            }
            Err(error) => {
                let detail = error.detail();
                self.errors.agents = Some(detail.clone());
                self.errors.projects = Some(detail);
            }
        }
        match snapshot.approvals {
            Ok(approvals) => {
                self.approvals = approvals;
                self.errors.approvals = None;
            }
            Err(error) => self.errors.approvals = Some(error.detail()),
        }
        match snapshot.events {
            Ok(events) => {
                self.events = events;
                self.errors.activity = None;
            }
            Err(error) => self.errors.activity = Some(error.detail()),
        }
        match snapshot.status {
            Ok(status) => {
                self.system = Some(status);
                self.errors.overview = None;
            }
            Err(error) => self.errors.overview = Some(error.detail()),
        }
        match snapshot.inbox {
            Ok(items) => {
                self.inbox_items = items;
                self.inbox_supported = true;
            }
            Err(error) => {
                if error.is_method_not_found() {
                    // v1 daemon: inbox simply does not exist yet.
                    self.inbox_supported = false;
                    self.inbox_items = Vec::new();
                } else {
                    self.errors.overview = Some(format!("inbox: {}", error.detail()));
                }
            }
        }

        self.loaded = true;
        self.connection = if unreachable >= 6 {
            Connection::Disconnected
        } else {
            Connection::Online
        };
        if self.connection == Connection::Online {
            self.last_refresh_ok = Some(Instant::now());
        } else if self.last_refresh_ok.is_none() {
            // Still nothing ever worked; keep Disconnected state visible.
        }

        self.next_refresh = Instant::now() + REFRESH_INTERVAL;
        self.refresh_started_at = None;
    }

    fn open_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| !matches!(task.status.as_str(), "done" | "canceled"))
            .count()
    }

    fn status_count(&self, status: &str) -> usize {
        self.tasks
            .iter()
            .filter(|task| task.status == status)
            .count()
    }

    fn active_agents(&self) -> usize {
        self.agents
            .managed
            .iter()
            .filter(|agent| matches!(agent.status.as_str(), "starting" | "running"))
            .count()
    }

    fn spinner_frame(&self) -> &'static str {
        let started = self.refresh_started_at.unwrap_or_else(Instant::now);
        let tick = (started.elapsed().as_millis() / 80) as usize;
        SPINNER_FRAMES[tick % SPINNER_FRAMES.len()]
    }

    fn refreshed_label(&self) -> String {
        match self.last_refresh_ok {
            Some(at) => format!("refreshed {} ago", humanize_elapsed(at.elapsed())),
            None => "never refreshed".to_string(),
        }
    }

    // -- composer -----------------------------------------------------------

    fn composer_submit(&mut self) {
        let text = self.composer_text.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.composer_text.clear();
        self.composer_focused = false;

        if let Some(rest) = text.strip_prefix('@') {
            match parse_indexed_command(rest) {
                Ok((n, message)) => match self.task_for_index(n) {
                    Some(task_id) => {
                        if message.is_empty() {
                            self.composer_note = Some(("usage: @N your message".into(), true));
                        } else {
                            self.composer_note = Some((format!("sending to {task_id}…"), false));
                            self.spawn_bg(move |client| {
                                BgResult::TaskMessaged(client.tasks_message(&task_id, &message))
                            });
                        }
                    }
                    None => {
                        self.composer_note = Some((format!(
                            "@{n} out of range — Tasks view is 1..{}",
                            self.tasks.len()
                        ), true));
                    }
                },
                Err(error) => self.composer_note = Some((error, true)),
            }
            return;
        }

        if let Some(command) = text.strip_prefix('/') {
            self.handle_slash_command(command.trim());
            return;
        }

        // Plain text → create a task.
        let note_task = truncate_chars(&text, 40);
        self.composer_note = Some((format!("creating “{note_task}”…"), false));
        self.spawn_bg(move |client| BgResult::TaskCreated(client.tasks_create(&text)));
    }

    fn task_for_index(&self, n: usize) -> Option<String> {
        if n == 0 {
            return None;
        }
        self.tasks.get(n - 1).map(|task| task.task_id.clone())
    }

    fn handle_slash_command(&mut self, command: &str) {
        let mut parts = command.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap_or("");
        let argument = parts.next().unwrap_or("").trim();
        match name {
            "brief" => {
                self.overlay = Some(Overlay::Brief {
                    state: OverlayState::Loading,
                });
                self.overlay_scroll = 0;
                self.spawn_bg(|client| BgResult::Brief(client.tasks_brief_global()));
            }
            "inbox" => {
                self.overlay = Some(Overlay::Inbox {
                    state: OverlayState::Loading,
                });
                self.overlay_scroll = 0;
                self.spawn_bg(|client| BgResult::Inbox(client.tasks_inbox()));
            }
            "tasks" => {
                self.overlay = None;
                self.selected = 2;
                self.composer_note = None;
            }
            "done" => self.mark_task(argument, "done"),
            "blocked" => self.mark_task(argument, "blocked"),
            _ => {
                self.composer_note = Some((
                    format!("unknown command /{name} — try /brief /inbox /tasks /done N /blocked N"),
                    true,
                ));
            }
        }
    }

    fn mark_task(&mut self, argument: &str, status: &'static str) {
        match argument.parse::<usize>() {
            Ok(n) => match self.task_for_index(n) {
                Some(task_id) => {
                    self.composer_note = Some((format!("marking {task_id} {status}…"), false));
                    self.spawn_bg(move |client| {
                        BgResult::TaskUpdated(client.tasks_update(&task_id, status))
                    });
                }
                None => {
                    self.composer_note = Some((format!(
                        "/{status} {n} out of range — Tasks view is 1..{}",
                        self.tasks.len()
                    ), true));
                }
            },
            Err(_) => {
                self.composer_note = Some((format!("usage: /{status} N"), true));
            }
        }
    }

    // -- key handling --------------------------------------------------------

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        let code = key.code;
        let modifiers = key.modifiers;

        // Overlays consume everything until closed.
        if self.overlay.is_some() {
            match code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.overlay = None;
                    self.overlay_scroll = 0;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.overlay_scroll = self.overlay_scroll.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.overlay_scroll = self.overlay_scroll.saturating_add(1);
                }
                _ => {}
            }
            return false;
        }

        // Composer: letters go to the input, Esc returns to navigation.
        if self.composer_focused {
            match code {
                KeyCode::Esc => self.composer_focused = false,
                KeyCode::Enter => self.composer_submit(),
                KeyCode::Backspace => {
                    self.composer_text.pop();
                }
                KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
                    self.composer_text.clear();
                }
                KeyCode::Char(c)
                    if !modifiers.contains(KeyModifiers::CONTROL)
                        && !modifiers.contains(KeyModifiers::ALT) =>
                {
                    self.composer_text.push(c);
                }
                KeyCode::Tab => self.composer_focused = false,
                _ => {}
            }
            return false;
        }

        if self.navigator_preview.is_some() {
            match code {
                KeyCode::Esc => self.navigator_preview = None,
                KeyCode::Enter => {
                    self.selected = self.navigator_preview.take().unwrap_or(self.selected);
                }
                KeyCode::Left | KeyCode::Up | KeyCode::Char('h') | KeyCode::Char('k') => {
                    self.move_section(-1)
                }
                KeyCode::Right | KeyCode::Down | KeyCode::Char('l') | KeyCode::Char('j') => {
                    self.move_section(1)
                }
                _ => {}
            }
            return false;
        }

        if code == KeyCode::Char('b') && modifiers.contains(KeyModifiers::CONTROL) {
            self.prefix_pending = true;
            return false;
        }

        if self.prefix_pending {
            self.prefix_pending = false;
            match code {
                KeyCode::Char('n') | KeyCode::Right if modifiers.contains(KeyModifiers::SHIFT) => {
                    self.move_section(1)
                }
                KeyCode::Char('p') | KeyCode::Left if modifiers.contains(KeyModifiers::SHIFT) => {
                    self.move_section(-1)
                }
                KeyCode::Char('n') => self.move_section(1),
                KeyCode::Char('p') => self.move_section(-1),
                KeyCode::Char('w') => self.navigator_preview = Some(self.selected),
                KeyCode::Tab if modifiers.contains(KeyModifiers::SHIFT) => self.move_panel(-1),
                KeyCode::Tab => self.move_panel(1),
                KeyCode::Char('z') => self.zoomed = !self.zoomed,
                _ => {}
            }
            return false;
        }

        match code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_section(1);
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_section(-1),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('n') => self.move_section(1),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('p') => self.move_section(-1),
            KeyCode::Tab => self.composer_focused = true,
            KeyCode::Char('1'..='6') => {
                if let KeyCode::Char(number) = code {
                    self.selected = (number as usize) - ('1' as usize);
                }
            }
            // Focus-by-typing: printable characters that are not navigation
            // bindings start composing. '/', '@' and plain text all land here.
            KeyCode::Char(c)
                if !modifiers.contains(KeyModifiers::CONTROL)
                    && !modifiers.contains(KeyModifiers::ALT)
                    && !matches!(c, 'q' | 'j' | 'k' | 'h' | 'l' | 'n' | 'p' | '1'..='6') =>
            {
                self.composer_focused = true;
                self.composer_text.push(c);
                self.composer_note = None;
            }
            _ => {}
        }
        false
    }

    fn handle_mouse(&mut self, kind: MouseEventKind, column: u16, row: u16, width: u16) {
        let in_workspace_chrome = if width < 112 {
            row == 3
        } else {
            column < 22 && row >= 3
        };
        if !in_workspace_chrome {
            return;
        }
        match kind {
            MouseEventKind::ScrollRight | MouseEventKind::ScrollDown => self.move_section(1),
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollUp => self.move_section(-1),
            _ => {}
        }
    }
}

fn parse_indexed_command(rest: &str) -> Result<(usize, String), String> {
    let mut parts = rest.trim().splitn(2, char::is_whitespace);
    let number = parts.next().unwrap_or("");
    let message = parts.next().unwrap_or("").trim().to_string();
    match number.parse::<usize>() {
        Ok(n) if n >= 1 => Ok((n, message)),
        _ => Err("usage: @N your message (N = row in the Tasks view)".into()),
    }
}

fn error_note(error: &RpcError) -> String {
    if error.is_method_not_found() {
        "daemon not updated yet".to_string()
    } else {
        error.detail()
    }
}

fn brief_error(error: &RpcError) -> String {
    if error.is_method_not_found() {
        "daemon not updated yet — this method ships in kbots-daemon v2".to_string()
    } else {
        error.detail()
    }
}

fn humanize_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

fn hhmmss(ts: &str) -> &str {
    if ts.len() >= 19 { &ts[11..19] } else { ts }
}

// ---------------------------------------------------------------------------
// Terminal lifecycle
// ---------------------------------------------------------------------------

struct MouseCaptureGuard;

impl MouseCaptureGuard {
    fn enable() -> io::Result<Self> {
        execute!(io::stdout(), EnableMouseCapture)?;
        Ok(Self)
    }
}

impl Drop for MouseCaptureGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
}

fn main() -> io::Result<()> {
    let mut app = App::new();
    ratatui::run(|terminal| {
        let _mouse_capture = MouseCaptureGuard::enable()?;
        let mut needs_draw = true;
        loop {
            needs_draw |= app.apply_pending();
            if app.refresh_due() {
                app.start_refresh();
                needs_draw = true;
            }
            if needs_draw {
                terminal.draw(|frame| render(frame, &app))?;
                needs_draw = false;
            }

            let timeout = if app.refreshing {
                Duration::from_millis(120) // animate the spinner
            } else {
                Duration::from_millis(1000) // keep "refreshed Xs ago" fresh
            };
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if app.handle_key(key) {
                            break Ok(());
                        }
                        // Every user action is followed by a retry/refresh,
                        // which also keeps a disconnected daemon retried.
                        app.request_refresh();
                        needs_draw = true;
                    }
                    Event::Mouse(mouse) => {
                        app.handle_mouse(
                            mouse.kind,
                            mouse.column,
                            mouse.row,
                            terminal.size()?.width,
                        );
                        needs_draw = true;
                    }
                    Event::Resize(_, _) => needs_draw = true,
                    _ => {}
                }
            } else if !app.refreshing {
                // Idle wake (once per second): repaint for the elapsed label.
                needs_draw = true;
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let banner = disconnected_banner(app);
    let note_visible = app.composer_note.is_some();

    let mut constraints = vec![Constraint::Length(3)];
    if banner.is_some() {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Min(0)); // body
    constraints.push(Constraint::Length(1)); // composer
    if note_visible {
        constraints.push(Constraint::Length(1)); // action note
    }
    constraints.push(Constraint::Length(1)); // footer

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut index = 0;
    render_header(frame, sections[index], app);
    index += 1;
    if let Some(banner) = banner {
        frame.render_widget(Paragraph::new(banner), sections[index]);
        index += 1;
    }
    render_body(frame, sections[index], app);
    index += 1;
    render_composer(frame, sections[index], app);
    index += 1;
    if note_visible {
        render_note(frame, sections[index], app);
        index += 1;
    }
    render_footer(frame, sections[index], app);

    if let Some(overlay) = &app.overlay {
        render_overlay(frame, area, overlay, app.overlay_scroll, app.refreshing);
    }
}

fn disconnected_banner(app: &App) -> Option<Line<'static>> {
    match app.connection {
        Connection::Disconnected => Some(Line::from(Span::styled(
            " daemon unreachable — kbots-daemon running? (retrying) ",
            Style::default().fg(Color::Black).bg(Color::Red).bold(),
        ))),
        Connection::Connecting => Some(Line::from(Span::styled(
            " connecting… ",
            Style::default().fg(Color::Black).bg(Color::Yellow).bold(),
        ))),
        Connection::Online => None,
    }
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let inbox_badge = if app.inbox_supported && !app.inbox_items.is_empty() {
        Some(Span::styled(
            format!("  Inbox {}  ", app.inbox_items.len()),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .bold(),
        ))
    } else {
        None
    };
    let state_span = match app.connection {
        Connection::Online => Span::styled(" LIVE ", Style::default().fg(Color::Black).bg(Color::Green).bold()),
        Connection::Connecting => {
            Span::styled(" connecting… ", Style::default().fg(Color::Black).bg(Color::Yellow).bold())
        }
        Connection::Disconnected => {
            Span::styled(" OFFLINE ", Style::default().fg(Color::White).bg(Color::Red).bold())
        }
    };

    let line = if area.width < 70 {
        let mut spans = vec![
            Span::styled(" kbots ", Style::default().fg(Color::Black).bg(Color::Green).bold()),
        ];
        if let Some(badge) = inbox_badge {
            spans.push(badge);
        }
        Line::from(spans)
    } else if area.width < 112 {
        let mut spans = vec![
            Span::styled(" kbots ", Style::default().fg(Color::Black).bg(Color::Green).bold()),
            Span::styled(" · Control Center ", Style::default().fg(Color::White).bold()),
        ];
        if let Some(badge) = inbox_badge {
            spans.push(badge);
        }
        spans.push(Span::raw(" "));
        spans.push(state_span);
        Line::from(spans)
    } else {
        let mut spans = vec![
            Span::styled(" kbots ", Style::default().fg(Color::Black).bg(Color::Green).bold()),
            Span::styled("  CONTROL CENTER  ", Style::default().fg(Color::White).bold()),
        ];
        if let Some(badge) = inbox_badge {
            spans.push(badge);
            spans.push(Span::raw(" "));
        }
        spans.push(state_span);
        Line::from(spans)
    };
    let header = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(header, area);
}

fn render_body(frame: &mut Frame, area: Rect, app: &App) {
    if app.zoomed && app.active_section() == 0 {
        render_focused_panel(frame, area, app);
        return;
    }
    if area.width < 112 {
        render_compact_body(frame, area, app);
        return;
    }

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(22), Constraint::Min(0)])
        .split(area);

    let items = NAVIGATION
        .iter()
        .enumerate()
        .map(|(index, item)| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {} ", index + 1),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(*item),
            ]))
        })
        .collect::<Vec<_>>();
    let navigation = List::new(items)
        .block(
            Block::default()
                .title(" WORKSPACE ")
                .borders(Borders::RIGHT),
        )
        .highlight_style(
            Style::default()
                .fg(Color::LightGreen)
                .bg(Color::Rgb(35, 49, 40))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut selected = ratatui::widgets::ListState::default();
    let active_section = app.active_section();
    selected.select(Some(active_section));
    frame.render_stateful_widget(navigation, columns[0], &mut selected);

    render_section(frame, columns[1], app, active_section);
}

fn render_compact_body(frame: &mut Frame, area: Rect, app: &App) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);
    let nav = Paragraph::new(Line::from(vec![
        Span::styled("‹ ", Style::default().fg(Color::LightGreen).bold()),
        Span::styled(
            format!(
                "{}/{}  {}",
                app.active_section() + 1,
                NAVIGATION.len(),
                NAVIGATION[app.active_section()]
            ),
            Style::default().fg(Color::White).bold(),
        ),
        Span::styled("  ›", Style::default().fg(Color::LightGreen).bold()),
    ]));
    frame.render_widget(nav, sections[0]);

    if sections[1].height < 8 && app.active_section() == 0 {
        let summary = Paragraph::new(vec![
            Line::from(Span::styled(
                format!(
                    "Projects {} · Tasks {} · Approvals {}",
                    app.agents.registered_projects,
                    app.tasks.len(),
                    app.approvals.len()
                ),
                Style::default().fg(Color::LightGreen).bold(),
            )),
            Line::from(Span::styled(
                "Enlarge terminal for activity details",
                Style::default().fg(Color::Gray),
            )),
        ]);
        frame.render_widget(summary, sections[1]);
        return;
    }

    render_section(frame, sections[1], app, app.active_section());
}

fn render_section(frame: &mut Frame, area: Rect, app: &App, section: usize) {
    match section {
        0 => render_overview(frame, area, app),
        1 => render_projects(frame, area, app),
        2 => render_tasks(frame, area, app),
        3 => render_agents(frame, area, app),
        4 => render_approvals(frame, area, app),
        5 => render_activity(frame, area, app),
        _ => render_overview(frame, area, app),
    }
}

/// Loading placeholder for sections whose data source has not answered yet.
/// `has_data` means a successful fetch has populated the section at least
/// once; `app.loaded` means at least one full snapshot completed (so an
/// empty section is genuinely empty, not still loading).
fn loading_or_error<'a>(
    app: &'a App,
    error: Option<&'a str>,
    has_data: bool,
    label: &str,
) -> Option<Line<'a>> {
    if has_data || app.loaded {
        return None;
    }
    if let Some(error) = error {
        return Some(Line::from(Span::styled(
            format!("{label}: {error}"),
            Style::default().fg(Color::Red),
        )));
    }
    let spinner = if app.refreshing {
        app.spinner_frame()
    } else {
        "·"
    };
    Some(Line::from(Span::styled(
        format!("{spinner} loading {label}…"),
        Style::default().fg(Color::DarkGray),
    )))
}

fn section_error_line<'a>(error: Option<&'a str>) -> Option<Line<'a>> {
    error.map(|error| {
        Line::from(Span::styled(
            format!("last refresh failed: {error}"),
            Style::default().fg(Color::Red),
        ))
    })
}

// -- Overview ----------------------------------------------------------------

fn render_overview(frame: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Min(10),
            Constraint::Length(7),
        ])
        .split(area);

    let subtitle = match &app.system {
        Some(status) => {
            let version = status
                .daemon
                .as_ref()
                .map(|daemon| format!("kbots-daemon v{}", daemon.version))
                .unwrap_or_else(|| "kbots-daemon".to_string());
            let uptime = status
                .daemon
                .as_ref()
                .map(|daemon| format!(" · up {}", humanize_elapsed(Duration::from_secs(daemon.uptime_seconds.max(0) as u64))))
                .unwrap_or_default();
            format!("{version}{uptime}")
        }
        None => {
            if app.connection == Connection::Disconnected {
                "daemon unreachable — showing last known state".to_string()
            } else {
                "connecting to daemon…".to_string()
            }
        }
    };
    let title = Paragraph::new(Line::from(vec![
        Span::styled("Overview", Style::default().fg(Color::White).bold()),
        Span::styled(format!("  {subtitle}"), Style::default().fg(Color::Gray)),
    ]))
    .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(title, rows[0]);

    render_metrics(frame, rows[1], app, app.focused_panel == 0);

    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(rows[2]);
    render_agents_panel(frame, panels[0], app, app.focused_panel == 1);
    render_tasks_panel(frame, panels[1], app, app.focused_panel == 2);
    render_events_panel(frame, rows[3], app, app.focused_panel == 3);
}

fn render_metrics(frame: &mut Frame, area: Rect, app: &App, focused: bool) {
    let metrics = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 4); 4])
        .split(area);

    let projects = if app.agents.registered_projects > 0 {
        app.agents.registered_projects as i64
    } else {
        app.system.as_ref().and_then(|s| s.projects).unwrap_or(0)
    };
    let open_tasks = app.open_count();
    let queued = app.status_count("queued");
    let running =
        app.status_count("running") + app.status_count("researching") + app.status_count("delegating");
    let active = app.active_agents();
    let approvals = app.approvals.len();

    let cards = [
        (
            "PROJECTS",
            projects.to_string(),
            format!("{} registered", projects),
            Color::Cyan,
        ),
        (
            "OPEN TASKS",
            open_tasks.to_string(),
            format!("{queued} queued · {running} in flight"),
            Color::LightGreen,
        ),
        (
            "ACTIVE AGENTS",
            active.to_string(),
            format!("{} managed · {} live", app.agents.managed.len(), app.agents.live_terminal_count()),
            Color::Magenta,
        ),
        (
            "APPROVALS",
            approvals.to_string(),
            "waiting for review".to_string(),
            Color::Yellow,
        ),
    ];
    for (index, (area, (label, value, detail, accent))) in metrics.iter().zip(cards).enumerate() {
        let card = Paragraph::new(vec![
            Line::from(Span::styled(label, Style::default().fg(Color::Gray).bold())),
            Line::from(vec![
                Span::styled(value, Style::default().fg(Color::White).bold()),
                Span::styled(format!("  {detail}"), Style::default().fg(accent)),
            ]),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(panel_border(focused && index == 0)),
        );
        frame.render_widget(card, *area);
    }
}

fn render_focused_panel(frame: &mut Frame, area: Rect, app: &App) {
    match app.focused_panel {
        0 => render_metrics(frame, area, app, true),
        1 => render_agents_panel(frame, area, app, true),
        2 => render_tasks_panel(frame, area, app, true),
        _ => render_events_panel(frame, area, app, true),
    }
}

fn panel_border(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(Color::LightGreen)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn managed_live_label(agent: &rpc::ManagedAgent) -> String {
    let Some(live) = agent.live.as_ref() else {
        return "—".to_string();
    };
    let state = live.get("state").and_then(serde_json::Value::as_str);
    let status = live
        .get("status")
        .and_then(|status| status.as_str().or_else(|| status.get("state").and_then(serde_json::Value::as_str)));
    match (state, status) {
        (Some(state), Some(status)) => format!("{state}:{status}"),
        (Some(state), None) => state.to_string(),
        (None, Some(status)) => status.to_string(),
        (None, None) => {
            if let Some(outcome) = live.get("outcome").and_then(serde_json::Value::as_str) {
                outcome.to_string()
            } else {
                "live".to_string()
            }
        }
    }
}

fn agent_status_style(status: &str) -> Style {
    match status {
        "running" | "starting" => Style::default().fg(Color::LightGreen),
        "stopping" => Style::default().fg(Color::Yellow),
        "error" => Style::default().fg(Color::Red),
        "stopped" => Style::default().fg(Color::DarkGray),
        _ => Style::default().fg(Color::Gray),
    }
}

fn render_agents_panel(frame: &mut Frame, area: Rect, app: &App, focused: bool) {
    let block = Block::default()
        .title(" Managed agents ")
        .borders(Borders::ALL)
        .border_style(panel_border(focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(loading) = loading_or_error(
        app,
        app.errors.agents.as_deref(),
        !app.agents.managed.is_empty(),
        "agents",
    ) {
        frame.render_widget(Paragraph::new(loading), inner);
        return;
    }

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:<10} {:<5} {:<10} {:<10} LIVE", "ID", "ROLE", "MODE", "STATUS"),
            Style::default().fg(Color::Gray).bold(),
        ),
    ]));
    for agent in app.agents.managed.iter().take(6) {
        let id = truncate_chars(&agent.id, 8);
        lines.push(Line::from(vec![
            Span::styled(format!("{id:<10} "), Style::default().fg(Color::Cyan)),
            Span::raw(format!("{:<5} ", truncate_chars(&agent.role, 4))),
            Span::raw(format!("{:<10} ", truncate_chars(&agent.mode, 8))),
            Span::styled(
                format!("{:<10} ", truncate_chars(&agent.status, 8)),
                agent_status_style(&agent.status),
            ),
            Span::styled(managed_live_label(agent), Style::default().fg(Color::Gray)),
        ]));
    }
    if app.agents.managed.is_empty() {
        lines.push(Line::from(Span::styled(
            "no managed agents — kbots agents start ceo",
            Style::default().fg(Color::DarkGray),
        )));
    }
    if let Some(error) = section_error_line(app.errors.agents.as_deref()) {
        lines.push(error);
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_tasks_panel(frame: &mut Frame, area: Rect, app: &App, focused: bool) {
    let block = Block::default()
        .title(" Open tasks ")
        .borders(Borders::ALL)
        .border_style(panel_border(focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(loading) = loading_or_error(
        app,
        app.errors.tasks.as_deref(),
        !app.tasks.is_empty(),
        "tasks",
    ) {
        frame.render_widget(Paragraph::new(loading), inner);
        return;
    }

    let mut lines = Vec::new();
    let open: Vec<&Task> = app
        .tasks
        .iter()
        .filter(|task| !matches!(task.status.as_str(), "done" | "canceled"))
        .take(4)
        .collect();
    if open.is_empty() {
        lines.push(Line::from(Span::styled(
            "no open tasks",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for task in open {
            lines.push(Line::from(vec![
                Span::styled(
                    truncate_chars(&task.task_id, 12),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(" "),
                Span::raw(truncate_chars(&task.title, 34)),
            ]));
            lines.push(Line::from(vec![
                Span::raw("           "),
                Span::styled(task.project_label(), Style::default().fg(Color::Magenta)),
                Span::raw(" · "),
                Span::styled(task.status.clone(), task_status_style(&task.status)),
            ]));
        }
    }
    if let Some(error) = section_error_line(app.errors.tasks.as_deref()) {
        lines.push(error);
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_events_panel(frame: &mut Frame, area: Rect, app: &App, focused: bool) {
    let block = Block::default()
        .title(" Recent events ")
        .borders(Borders::ALL)
        .border_style(panel_border(focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(loading) = loading_or_error(
        app,
        app.errors.activity.as_deref(),
        !app.events.is_empty(),
        "events",
    ) {
        frame.render_widget(Paragraph::new(loading), inner);
        return;
    }

    let mut lines = event_lines(app.events.iter().take(4));
    if app.events.is_empty() {
        lines.push(Line::from(Span::styled(
            "no events yet",
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

// -- Projects ----------------------------------------------------------------

struct ProjectSummary {
    name: String,
    path: String,
    managed: usize,
    registered_items: usize,
}

fn collect_projects(app: &App) -> BTreeMap<String, ProjectSummary> {
    let mut map: BTreeMap<String, ProjectSummary> = BTreeMap::new();
    for item in &app.agents.items {
        let key = item.project_id.clone().unwrap_or_else(|| item.project_path.clone());
        let entry = map.entry(key).or_insert_with(|| ProjectSummary {
            name: item.project_name.clone().unwrap_or_else(|| {
                item.project_path
                    .rsplit('/')
                    .find(|s| !s.is_empty())
                    .unwrap_or("?")
                    .to_string()
            }),
            path: item.project_path.clone(),
            managed: 0,
            registered_items: 0,
        });
        entry.registered_items += 1;
    }
    for agent in &app.agents.managed {
        let key = agent.project_id.clone().unwrap_or_else(|| "unassigned".to_string());
        let entry = map.entry(key).or_insert_with(|| ProjectSummary {
            name: agent.project_name.clone().unwrap_or_else(|| {
                agent
                    .project_id
                    .clone()
                    .unwrap_or_else(|| "unassigned".to_string())
            }),
            path: String::new(),
            managed: 0,
            registered_items: 0,
        });
        entry.managed += 1;
    }
    map
}

fn render_projects(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .title(" Projects ")
        .borders(Borders::ALL)
        .border_style(panel_border(false));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(loading) = loading_or_error(
        app,
        app.errors.projects.as_deref(),
        !app.agents.items.is_empty() || !app.agents.managed.is_empty(),
        "projects",
    ) {
        frame.render_widget(Paragraph::new(loading), inner);
        return;
    }

    let projects = collect_projects(app);
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            format!("registeredProjects: {}", app.agents.registered_projects),
            Style::default().fg(Color::Gray),
        ),
        Span::raw("   "),
        Span::styled(
            format!("managed agents: {}", app.agents.managed.len()),
            Style::default().fg(Color::Gray),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        format!("{:<20} {:<40} {:<8} {:<8}", "PROJECT", "PATH", "SLOTS", "MANAGED"),
        Style::default().fg(Color::Gray).bold(),
    )));
    if projects.is_empty() {
        lines.push(Line::from(Span::styled(
            "no registered projects — kbots project add <path>",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for project in projects.values() {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<20} ", truncate_chars(&project.name, 19)),
                Style::default().fg(Color::Cyan).bold(),
            ),
            Span::raw(format!("{:<40} ", truncate_chars(&project.path, 39))),
            Span::raw(format!("{:<8} ", project.registered_items)),
            Span::styled(
                format!("{:<8}", project.managed),
                if project.managed > 0 {
                    Style::default().fg(Color::LightGreen)
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            ),
        ]));
    }
    if let Some(error) = section_error_line(app.errors.projects.as_deref()) {
        lines.push(error);
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

// -- Tasks -------------------------------------------------------------------

fn task_status_style(status: &str) -> Style {
    match status {
        "queued" => Style::default().fg(Color::Gray),
        "researching" | "planning" => Style::default().fg(Color::Cyan),
        "delegating" | "running" => Style::default().fg(Color::LightBlue),
        "review" => Style::default().fg(Color::Yellow),
        "blocked" => Style::default().fg(Color::Red),
        "done" => Style::default().fg(Color::LightGreen),
        "failed" => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        "canceled" => Style::default().fg(Color::DarkGray),
        _ => Style::default().fg(Color::White),
    }
}

fn render_tasks(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .title(format!(
            " Tasks · {} rows · @N message · /done N · /blocked N ",
            app.tasks.len()
        ))
        .borders(Borders::ALL)
        .border_style(panel_border(false));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(loading) = loading_or_error(
        app,
        app.errors.tasks.as_deref(),
        !app.tasks.is_empty(),
        "tasks",
    ) {
        frame.render_widget(Paragraph::new(loading), inner);
        return;
    }

    if app.tasks.is_empty() {
        let line = section_error_line(app.errors.tasks.as_deref()).unwrap_or_else(|| {
            Line::from(Span::styled(
                "no tasks yet — type a title below and press Enter",
                Style::default().fg(Color::DarkGray),
            ))
        });
        frame.render_widget(Paragraph::new(line), inner);
        return;
    }

    let (table_area, error_area) = match section_error_line(app.errors.tasks.as_deref()) {
        Some(error) => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(0), Constraint::Length(1)])
                .split(inner);
            (chunks[0], Some((error, chunks[1])))
        }
        None => (inner, None),
    };

    let visible = table_area.height.saturating_sub(1) as usize;
    let rows = app.tasks.iter().take(visible.max(1)).enumerate().map(|(index, task)| {
        let ceo = task
            .ceo_agent_id
            .as_deref()
            .map(|id| truncate_chars(id, 8))
            .unwrap_or_else(|| "—".to_string());
        Row::new(vec![
            format!("{:>2}", index + 1),
            truncate_chars(&task.task_id, 10),
            truncate_chars(&task.title, 44),
            task.status.clone(),
            truncate_chars(&task.project_label(), 16),
            ceo,
        ])
        .style(task_status_style(&task.status))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Length(11),
            Constraint::Min(20),
            Constraint::Length(11),
            Constraint::Length(17),
            Constraint::Length(9),
        ],
    )
    .header(
        Row::new(["#", "ID", "TITLE", "STATUS", "PROJECT", "CEO"])
            .style(Style::default().fg(Color::Gray).bold()),
    )
    .column_spacing(1);
    frame.render_widget(table, table_area);
    if let Some((error, area)) = error_area {
        frame.render_widget(Paragraph::new(error), area);
    }
}

// -- Agents -----------------------------------------------------------------

fn render_agents(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .title(format!(
            " Managed agents · {} live terminals ",
            app.agents.live_terminal_count()
        ))
        .borders(Borders::ALL)
        .border_style(panel_border(false));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(loading) = loading_or_error(
        app,
        app.errors.agents.as_deref(),
        !app.agents.managed.is_empty() || !app.agents.items.is_empty(),
        "agents",
    ) {
        frame.render_widget(Paragraph::new(loading), inner);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    // Group managed agents by project, preserving name sort for stability.
    let mut groups: BTreeMap<String, Vec<&rpc::ManagedAgent>> = BTreeMap::new();
    for agent in &app.agents.managed {
        let key = agent
            .project_name
            .clone()
            .or_else(|| agent.project_id.clone())
            .unwrap_or_else(|| "unassigned".to_string());
        groups.entry(key).or_default().push(agent);
    }

    lines.push(Line::from(Span::styled(
        format!("{:<10} {:<6} {:<9} {:<9} HERDR", "ID", "ROLE", "MODE", "STATUS"),
        Style::default().fg(Color::Gray).bold(),
    )));
    if groups.is_empty() {
        lines.push(Line::from(Span::styled(
            "no managed agents — kbots agents start ceo --project <ref>",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (project, agents) in groups {
        lines.push(Line::from(vec![
            Span::styled(
                format!("▌ {project}"),
                Style::default().fg(Color::Magenta).bold(),
            ),
        ]));
        for agent in agents {
            let id = truncate_chars(&agent.id, 8);
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{id:<9} "), Style::default().fg(Color::Cyan)),
                Span::raw(format!("{:<6} ", truncate_chars(&agent.role, 5))),
                Span::raw(format!("{:<9} ", truncate_chars(&agent.mode, 8))),
                Span::styled(
                    format!("{:<9} ", truncate_chars(&agent.status, 8)),
                    agent_status_style(&agent.status),
                ),
                Span::styled(managed_live_label(agent), Style::default().fg(Color::Gray)),
            ]));
            if let Some(error) = agent.last_error.as_deref() {
                lines.push(Line::from(vec![
                    Span::raw("     "),
                    Span::styled(
                        format!("lastError: {}", truncate_chars(error, 60)),
                        Style::default().fg(Color::Red),
                    ),
                ]));
            }
        }
    }

    // Unmanaged role slots with a herdr attachment, if any.
    for item in app.agents.items.iter().filter(|item| item.herdr.is_some()) {
        let herdr = item.herdr.as_ref().expect("filtered");
        let project = item
            .project_name
            .clone()
            .or_else(|| item.project_path.rsplit('/').find(|s| !s.is_empty()).map(str::to_string))
            .unwrap_or_else(|| "?".to_string());
        let state = herdr
            .state
            .as_deref()
            .or(herdr.status.as_deref())
            .unwrap_or("attached");
        lines.push(Line::from(vec![
            Span::styled(format!("▌ {project} "), Style::default().fg(Color::Magenta)),
            Span::styled(&item.role, Style::default().fg(Color::Yellow)),
            Span::raw(" · herdr "),
            Span::styled(state.to_string(), Style::default().fg(Color::Gray)),
        ]));
    }

    lines.push(Line::from(Span::styled(
        format!(
            "liveTerminals: {} · filteredOut: {}{}",
            app.agents.live_terminal_count(),
            app.agents.filtered_out,
            app.agents
                .herdr_unavailable
                .as_deref()
                .map(|msg| format!(" · herdr unavailable: {msg}"))
                .unwrap_or_default()
        ),
        Style::default().fg(Color::DarkGray),
    )));
    if let Some(error) = section_error_line(app.errors.agents.as_deref()) {
        lines.push(error);
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

// -- Approvals ---------------------------------------------------------------

fn render_approvals(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .title(format!(" Approvals · {} pending ", app.approvals.len()))
        .borders(Borders::ALL)
        .border_style(panel_border(false));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(loading) = loading_or_error(
        app,
        app.errors.approvals.as_deref(),
        !app.approvals.is_empty(),
        "approvals",
    ) {
        frame.render_widget(Paragraph::new(loading), inner);
        return;
    }

    if app.approvals.is_empty() {
        let line = section_error_line(app.errors.approvals.as_deref()).unwrap_or_else(|| {
            Line::from(Span::styled(
                "no pending approvals ✓",
                Style::default().fg(Color::LightGreen),
            ))
        });
        frame.render_widget(Paragraph::new(line), inner);
        return;
    }

    let (table_area, error_area) = match section_error_line(app.errors.approvals.as_deref()) {
        Some(error) => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(0), Constraint::Length(1)])
                .split(inner);
            (chunks[0], Some((error, chunks[1])))
        }
        None => (inner, None),
    };

    let visible = table_area.height.saturating_sub(1) as usize;
    let rows = app.approvals.iter().take(visible.max(1)).map(|approval| {
        Row::new(vec![
            truncate_chars(&approval.approval_id, 10),
            truncate_chars(&approval.operation, 18),
            truncate_chars(
                approval
                    .project_id
                    .as_deref()
                    .map(str::to_string)
                    .unwrap_or_else(|| "—".to_string())
                    .as_str(),
                12,
            ),
            truncate_chars(approval.detail.as_deref().unwrap_or("—"), 48),
            approval
                .created_at
                .as_deref()
                .map(hhmmss)
                .unwrap_or("—")
                .to_string(),
        ])
        .style(Style::default().fg(Color::Yellow))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(11),
            Constraint::Length(19),
            Constraint::Length(13),
            Constraint::Min(20),
            Constraint::Length(9),
        ],
    )
    .header(
        Row::new(["ID", "OPERATION", "PROJECT", "DETAIL", "CREATED"])
            .style(Style::default().fg(Color::Gray).bold()),
    )
    .column_spacing(1);
    frame.render_widget(table, table_area);
    if let Some((error, area)) = error_area {
        frame.render_widget(Paragraph::new(error), area);
    }
}

// -- Activity log ------------------------------------------------------------

fn event_kind_style(kind: &str) -> Style {
    let kind = kind.to_ascii_lowercase();
    if kind.contains("fail") || kind.contains("error") || kind.contains("blocked") {
        Style::default().fg(Color::Red)
    } else if kind.contains("approval") {
        Style::default().fg(Color::Yellow)
    } else if kind.contains("dispatch") || kind.contains("start") {
        Style::default().fg(Color::LightBlue)
    } else if kind.contains("done") || kind.contains("completed") || kind.contains("created") {
        Style::default().fg(Color::LightGreen)
    } else {
        Style::default().fg(Color::Cyan)
    }
}

fn event_lines<'a>(events: impl Iterator<Item = &'a EventItem>) -> Vec<Line<'a>> {
    events
        .map(|event| {
            Line::from(vec![
                Span::styled(
                    hhmmss(&event.ts).to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw("  "),
                Span::styled(
                    truncate_chars(&event.kind, 26),
                    event_kind_style(&event.kind),
                ),
                Span::raw("  "),
                Span::raw(truncate_chars(&event.message, 90)),
            ])
        })
        .collect()
}

fn render_activity(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .title(" Activity · newest first ")
        .borders(Borders::ALL)
        .border_style(panel_border(false));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(loading) = loading_or_error(
        app,
        app.errors.activity.as_deref(),
        !app.events.is_empty(),
        "events",
    ) {
        frame.render_widget(Paragraph::new(loading), inner);
        return;
    }

    let mut lines = Vec::new();
    if app.events.is_empty() {
        lines.push(Line::from(Span::styled(
            "no events recorded yet",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let limit = inner.height as usize;
        lines.extend(event_lines(app.events.iter().take(limit.max(1))));
    }
    if let Some(error) = section_error_line(app.errors.activity.as_deref()) {
        lines.push(error);
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

// -- Composer / footer -------------------------------------------------------

fn render_composer(frame: &mut Frame, area: Rect, app: &App) {
    let line = if app.composer_focused {
        Line::from(vec![
            Span::styled(" › ", Style::default().fg(Color::LightGreen).bold()),
            Span::styled(app.composer_text.clone(), Style::default().fg(Color::White).bold()),
            Span::styled("▌", Style::default().fg(Color::LightGreen)),
            Span::styled(
                "   Enter send · Esc back to nav",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else if app.composer_text.is_empty() {
        Line::from(vec![
            Span::styled(" › ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "type a task, @N to message, /brief /inbox — Esc nav",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(" › ", Style::default().fg(Color::DarkGray)),
            Span::styled(app.composer_text.clone(), Style::default().fg(Color::White)),
        ])
    };
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(Color::Rgb(20, 24, 22))),
        area,
    );
}

fn render_note(frame: &mut Frame, area: Rect, app: &App) {
    let Some((text, is_error)) = &app.composer_note else {
        return;
    };
    let style = if *is_error {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::LightGreen)
    };
    frame.render_widget(Paragraph::new(Line::from(Span::styled(format!(" {text}"), style))), area);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    let left = if area.width < 70 {
        Line::from(vec![
            Span::styled("n/p ", Style::default().fg(Color::LightGreen).bold()),
            Span::raw("sections · Tab composer · q quit"),
        ])
    } else if area.width < 112 {
        Line::from(vec![
            Span::styled("n/p ", Style::default().fg(Color::LightGreen).bold()),
            Span::raw("sections · Ctrl+B chords · Tab type · q quit"),
        ])
    } else {
        Line::from(vec![
            Span::styled("←/→ n/p ", Style::default().fg(Color::LightGreen).bold()),
            Span::raw("sections · Ctrl+B n/p w Tab z · type to compose · q/Esc quit"),
        ])
    };

    let status_text = if app.refreshing {
        format!("{} refreshing…", app.spinner_frame())
    } else {
        app.refreshed_label()
    };
    let status_style = if app.connection == Connection::Disconnected {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(28)])
        .split(area);
    frame.render_widget(Paragraph::new(left), chunks[0]);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(status_text, status_style))).alignment(Alignment::Right),
        chunks[1],
    );
}

// -- Overlays ----------------------------------------------------------------

fn parse_brief(value: &serde_json::Value) -> BriefView {
    let generated_at = value
        .get("generatedAt")
        .or_else(|| value.get("generated_at"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();

    let mut counts = Vec::new();
    let mut total_label = None;
    match value.get("counts") {
        Some(serde_json::Value::Object(map)) => {
            for (key, count) in map {
                counts.push((key.clone(), rpc::render_value(count)));
            }
        }
        Some(serde_json::Value::Array(items)) => {
            for (index, item) in items.iter().enumerate() {
                counts.push((format!("entry {index}"), rpc::render_value(item)));
            }
        }
        Some(other) => total_label = Some(rpc::render_value(other)),
        None => {}
    }

    let collect = |key: &str| -> Vec<String> {
        value
            .get(key)
            .and_then(serde_json::Value::as_array)
            .map(|items| items.iter().map(brief_entry_line).collect())
            .unwrap_or_default()
    };

    BriefView {
        generated_at,
        counts,
        total_label,
        needs_action: collect("needsAction"),
        running: collect("running"),
        done: collect("done"),
    }
}

fn overlay_area(area: Rect) -> Rect {
    let width = area.width.saturating_sub(8).clamp(20, 88);
    let height = area.height.saturating_sub(4).clamp(6, 22);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

fn render_overlay(frame: &mut Frame, area: Rect, overlay: &Overlay, scroll: usize, refreshing: bool) {
    let popup = overlay_area(area);
    frame.render_widget(Clear, popup);

    let lines: Vec<Line> = match overlay {
        Overlay::Brief { state } => match state {
            OverlayState::Loading => vec![loading_line("loading brief", refreshing)],
            OverlayState::Failed(error) => failed_lines(error),
            OverlayState::Ready(brief) => brief_lines(brief),
        },
        Overlay::Inbox { state } => match state {
            OverlayState::Loading => vec![loading_line("loading inbox", refreshing)],
            OverlayState::Failed(error) => failed_lines(error),
            OverlayState::Ready(items) => inbox_lines(items),
        },
    };

    let hint = Line::from(Span::styled(
        "j/k scroll · Esc close",
        Style::default().fg(Color::DarkGray),
    ));
    let body = popup.height.saturating_sub(2).max(1) as usize;
    // Reserve the last row for the hint when content overflows.
    let content_rows = if lines.len() > body { body.saturating_sub(1).max(1) } else { body };

    let block = Block::default()
        .title(overlay.title())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightGreen));
    frame.render_widget(block, popup);
    let inner = Rect {
        x: popup.x + 1,
        y: popup.y + 1,
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };
    let start = if lines.len() > content_rows {
        scroll.min(lines.len().saturating_sub(content_rows))
    } else {
        0
    };
    let mut all: Vec<Line> = lines
        .into_iter()
        .skip(start)
        .take(content_rows)
        .collect();
    all.push(hint);
    frame.render_widget(Paragraph::new(all), inner);
}

fn loading_line(label: &str, _refreshing: bool) -> Line<'static> {
    Line::from(Span::styled(
        format!("⠋ {label}…"),
        Style::default().fg(Color::DarkGray),
    ))
}

fn failed_lines(error: &str) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            "could not load this view",
            Style::default().fg(Color::Red).bold(),
        )),
        Line::from(Span::styled(error.to_string(), Style::default().fg(Color::Yellow))),
    ]
}

fn brief_lines(brief: &BriefView) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if !brief.generated_at.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("generated ", Style::default().fg(Color::DarkGray)),
            Span::styled(brief.generated_at.clone(), Style::default().fg(Color::Gray)),
        ]));
        lines.push(Line::default());
    }
    lines.push(Line::from(Span::styled(
        "COUNTS",
        Style::default().fg(Color::Gray).bold(),
    )));
    if brief.counts.is_empty() {
        match &brief.total_label {
            Some(total) => lines.push(Line::from(format!("  {total}"))),
            None => lines.push(Line::from(Span::styled(
                "  (no counts reported)",
                Style::default().fg(Color::DarkGray),
            ))),
        }
    } else {
        for (key, value) in &brief.counts {
            lines.push(Line::from(format!("  {key}: {value}")));
        }
    }
    lines.push(Line::default());
    push_brief_section(&mut lines, "NEEDS ACTION", &brief.needs_action, Color::Yellow);
    push_brief_section(&mut lines, "RUNNING", &brief.running, Color::LightBlue);
    push_brief_section(&mut lines, "DONE TODAY", &brief.done, Color::LightGreen);
    lines
}

fn push_brief_section(lines: &mut Vec<Line<'static>>, label: &str, items: &[String], accent: Color) {
    lines.push(Line::from(Span::styled(
        label.to_string(),
        Style::default().fg(accent).bold(),
    )));
    if items.is_empty() {
        lines.push(Line::from(Span::styled(
            "  —",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for item in items {
            lines.push(Line::from(format!("  {item}")));
        }
    }
    lines.push(Line::default());
}

fn inbox_lines(items: &[InboxItem]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if items.is_empty() {
        lines.push(Line::from(Span::styled(
            "inbox zero — nothing needs you right now ✓",
            Style::default().fg(Color::LightGreen),
        )));
        return lines;
    }

    // Group by kind: known kinds in fixed order, then any unknown kinds.
    let mut kinds: Vec<String> = INBOX_KIND_ORDER.iter().map(|k| k.to_string()).collect();
    for item in items {
        if !kinds.iter().any(|kind| kind == &item.kind) {
            kinds.push(item.kind.clone());
        }
    }

    for kind in kinds {
        let group: Vec<&InboxItem> = items.iter().filter(|item| item.kind == kind).collect();
        if group.is_empty() {
            continue;
        }
        let accent = inbox_kind_style(&kind);
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}  ({})", kind.to_uppercase(), group.len()),
                Style::default().fg(accent).bold(),
            ),
        ]));
        for item in group {
            let task = item.task_id.clone().unwrap_or_else(|| "—".to_string());
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<10} ", truncate_chars(&task, 9)),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(item.summary()),
            ]));
        }
        lines.push(Line::default());
    }
    lines
}

fn inbox_kind_style(kind: &str) -> Color {
    match kind {
        "approval" => Color::Yellow,
        "blocked" => Color::Red,
        "failed" => Color::Red,
        "queued" => Color::Gray,
        "review" => Color::Yellow,
        _ => Color::Cyan,
    }
}
