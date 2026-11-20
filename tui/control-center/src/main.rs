mod codex_term;
mod rpc;

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEventKind,
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
    TaskBrief, TaskMessage, brief_entry_line, truncate_chars,
};

use codex_term::{CodexTerminals, key_to_pty_bytes};

const NAVIGATION: [&str; 7] = [
    "Codex",
    "Overview",
    "Projects",
    "Tasks",
    "Agents",
    "Approvals",
    "Activity log",
];

const CODEX_SECTION: usize = 0;

const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
/// How long a Simple-mode Codex exit screen stays visible before the TUI
/// follows the process out.
const CODEX_EXIT_GRACE: Duration = Duration::from_millis(1500);
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
    TaskMessaged {
        result: Result<rpc::TaskMessageResult, RpcError>,
    },
    TaskUpdated(Result<rpc::TaskUpdateResult, RpcError>),
    TaskDetail(Box<TaskDetailPayload>),
}

struct TaskDetailPayload {
    task_id: String,
    brief: Result<TaskBrief, RpcError>,
    messages: Result<Vec<TaskMessage>, RpcError>,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Default: only the embedded Codex terminal with a minimal header.
    Simple,
    /// `--full`: the complete 7-section operator dashboard.
    Full,
}

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
    Inbox { state: OverlayState<Vec<InboxItem>>, selected: usize },
    TaskDetail {
        state: Box<OverlayState<TaskDetailView>>,
        task_id: String,
    },
}

impl Overlay {
    fn title(&self) -> &'static str {
        match self {
            Overlay::Brief { .. } => " /brief ",
            Overlay::Inbox { .. } => " /inbox ",
            Overlay::TaskDetail { .. } => " /task ",
        }
    }

    fn hint(&self) -> &'static str {
        match self {
            Overlay::Brief { .. } => "j/k scroll · Esc close",
            Overlay::Inbox { .. } => "n/p move · Enter open · Esc close",
            Overlay::TaskDetail { .. } => "Esc close · d done · b blocked · n message · j/k scroll",
        }
    }
}

/// Everything the task detail overlay renders; per-section errors are kept
/// inline so a half-updated daemon never blanks the whole view.
struct TaskDetailView {
    title: String,
    status: String,
    project: String,
    summary: Option<String>,
    decisions: Vec<String>,
    blockers: Vec<String>,
    next_step: Option<String>,
    ceo: Option<rpc::TaskBriefCeo>,
    pending_messages: Option<u64>,
    messages: Vec<TaskMessage>, // newest first, at most 4
    brief_error: Option<String>,
    messages_error: Option<String>,
}

struct App {
    mode: Mode,
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
    codex_terms: CodexTerminals,
    /// Binding for the embedded Codex terminal: None = free session, Some(i)
    /// = bound to the i-th open task in list order.
    codex_binding: Option<usize>,
    codex_spawn_error: Option<String>,
    /// Keys of terminals whose process has exited (screen stays visible).
    codex_exited: Vec<String>,
    /// Set by mouse-click exit in Simple mode and by the embedded Codex
    /// process exiting; checked by the frame loop.
    quit: bool,
    /// Simple-mode Codex exited: quit once this instant passes (grace so a
    /// fast crash's final screen is readable before the TUI closes).
    quit_at: Option<Instant>,
}

impl App {
    fn new(mode: Mode) -> Self {
        let (bg_tx, bg_rx) = mpsc::channel();
        Self {
            mode,
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
            codex_terms: CodexTerminals::default(),
            codex_binding: None,
            codex_spawn_error: None,
            codex_exited: Vec::new(),
            quit: false,
            quit_at: None,
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
                        let ordered = inbox_display_items(&items);
                        self.inbox_items = ordered.clone();
                        self.inbox_supported = true;
                        OverlayState::Ready(ordered)
                    }
                    Err(error) => OverlayState::Failed(brief_error(&error)),
                };
                self.overlay = Some(Overlay::Inbox { state, selected: 0 });
                self.overlay_scroll = 0;
            }
            BgResult::TaskDetail(payload) => {
                let TaskDetailPayload {
                    task_id,
                    brief,
                    messages,
                } = *payload;
                let still_open = matches!(
                    &self.overlay,
                    Some(Overlay::TaskDetail { task_id: open, .. }) if *open == task_id
                );
                if still_open {
                    let project = self
                        .tasks
                        .iter()
                        .find(|task| task.task_id == task_id)
                        .map(Task::project_label);
                    let task_title = self
                        .tasks
                        .iter()
                        .find(|task| task.task_id == task_id)
                        .map(|task| task.title.clone());
                    let view = build_task_detail(task_id.clone(), brief, messages, project, task_title);
                    self.overlay = Some(Overlay::TaskDetail {
                        state: Box::new(OverlayState::Ready(view)),
                        task_id,
                    });
                    self.overlay_scroll = 0;
                }
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
            BgResult::TaskMessaged { result } => {
                let note = match result {
                    Ok(sent) => {
                        if sent.outcome == "delivered" {
                            let via = sent
                                .message_id
                                .as_deref()
                                .map(|id| format!(" (msg {})", truncate_chars(id, 10)))
                                .unwrap_or_default();
                            (format!("Sent to CEO of {}{}", sent.task_id, via), false)
                        } else {
                            (format!("Queued for {} (CEO busy)", sent.task_id), false)
                        }
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
                self.inbox_items = inbox_display_items(&items);
                self.inbox_supported = true;
                // Keep an open inbox overlay's selection valid.
                if let Some(Overlay::Inbox {
                    state: OverlayState::Ready(items),
                    selected,
                }) = &mut self.overlay
                    && !items.is_empty()
                    && *selected >= items.len()
                {
                    *selected = items.len() - 1;
                }
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
                                BgResult::TaskMessaged {
                                    result: client.tasks_message(&task_id, &message),
                                }
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

        self.composer_note = Some((
            "plain text goes to Codex — press 1 for the Codex page".to_string(),
            true,
        ));
    }

    fn open_tasks(&self) -> Vec<(String, String, String)> {
        self.tasks
            .iter()
            .filter(|task| !matches!(task.status.as_str(), "done" | "canceled"))
            .map(|task| {
                (
                    task.task_id.clone(),
                    task.project_path.clone().unwrap_or_default(),
                    task.title.clone(),
                )
            })
            .collect()
    }

    fn codex_binding_info(&self) -> Option<(String, String, String)> {
        let index = self.codex_binding?;
        self.open_tasks().get(index).cloned()
    }

    fn codex_binding_key(&self) -> String {
        self.codex_binding_info()
            .map(|(id, _, _)| id)
            .unwrap_or_else(|| "free".to_string())
    }

    fn codex_binding_label(&self) -> String {
        match &self.codex_binding_info() {
            Some((id, _, title)) => format!("{id} · {}", truncate_chars(title, 48)),
            None => "free · no task bound".to_string(),
        }
    }

    fn cycle_codex_binding(&mut self, offset: isize) {
        let count = self.open_tasks().len() as isize;
        let current = self.codex_binding.map(|i| i as isize + 1).unwrap_or(0);
        let next = (current + offset).rem_euclid(count + 1);
        self.codex_binding = if next == 0 { None } else { Some((next - 1) as usize) };
    }

    /// Walk from `start` up to (and including) the enclosing git root and
    /// return the git root plus the `.agents/bot/router` folder when it
    /// exists there. Never escapes the repository.
    fn find_router_dir(start: &Path) -> Option<(PathBuf, PathBuf)> {
        let mut dir = start.to_path_buf();
        loop {
            let candidate = dir.join(".agents").join("bot").join("router");
            if candidate.is_dir() {
                return Some((dir, candidate));
            }
            let at_git_root = dir.join(".git").is_dir() || dir.join(".git").is_file();
            if at_git_root || !dir.pop() {
                return None;
            }
        }
    }

    /// Locate the router folder for `start`; when missing, create it from
    /// the embedded template (same as `kb project sync` for this role).
    fn resolve_or_scaffold_router(start: &Path) -> Option<PathBuf> {
        if let Some((_, router)) = Self::find_router_dir(start) {
            return Some(router);
        }
        // No router folder yet. Scaffold one at the enclosing git root.
        let mut dir = start.to_path_buf();
        loop {
            let at_git_root = dir.join(".git").is_dir() || dir.join(".git").is_file();
            if at_git_root {
                match codex_term::scaffold_router_folder(&dir) {
                    Ok(created) => return Some(created),
                    Err(_) => return None,
                }
            }
            if !dir.pop() {
                return None;
            }
        }
    }

    fn codex_cwd(&self) -> PathBuf {
        // Codex runs inside the project's router bot folder when it exists —
        // that's where the task-router instructions live (see
        // `kb project add` / `kb project sync` scaffolding).
        let info = self.codex_binding_info();
        let project_root = info
            .as_ref()
            .map(|(_, path, _)| PathBuf::from(path))
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
            });
        if let Some(found) = Self::resolve_or_scaffold_router(&project_root) {
            return found;
        }
        if let Ok(cwd) = std::env::current_dir()
            && let Some(found) = Self::resolve_or_scaffold_router(&cwd)
        {
            return found;
        }
        project_root
    }

    fn spawn_codex(&mut self, key: &str) {
        let info = self.codex_binding_info();
        let cwd = self.codex_cwd();
        let context = info.map(|(id, _, title)| {
            format!("Continue the kbots task {id}: {title}. Work in this repository.")
        });
        match codex_term::CodexTerm::spawn(24, 80, &cwd, context.as_deref()) {
            Ok(term) => {
                self.codex_terms.insert(key.to_string(), term);
                self.codex_exited.retain(|k| k != key);
                self.quit_at = None;
                self.codex_spawn_error = None;
            }
            Err(error) => {
                self.codex_spawn_error = Some(format!("cannot spawn codex: {error}"));
            }
        }
    }

    fn respawn_codex(&mut self, key: &str) {
        self.codex_terms.remove(key);
        self.codex_exited.retain(|k| k != key);
        self.codex_spawn_error = None;
        self.spawn_codex(key);
    }

    /// Reap exited processes and spawn the bound terminal if missing.
    /// Called from the frame loop while the Codex page is active.
    fn ensure_codex(&mut self) {
        if self.active_section() != CODEX_SECTION {
            return;
        }
        let key = self.codex_binding_key();
        if let Some(term) = self.codex_terms.get_mut(&key)
            && term.has_exited()
            && !self.codex_exited.iter().any(|k| k == &key)
        {
            self.codex_exited.push(key.clone());
            // Simple mode *is* the Codex pane; when the process ends there
            // is nothing left to do, so follow it out (same as Ctrl+]) after
            // a short grace that keeps a crash's last screen readable.
            // --full keeps the page and just marks it exited.
            if self.mode == Mode::Simple {
                self.quit_at = Some(Instant::now() + CODEX_EXIT_GRACE);
            }
        }
        if let Ok((cols, rows)) = crossterm::terminal::size() {
            // Sync the pty to roughly the body size (header/footer/borders on top).
            let pty_rows = rows.saturating_sub(6).max(1);
            let pty_cols = cols.saturating_sub(2).max(1);
            if let Some(term) = self.codex_terms.get_mut(&key) {
                term.resize(pty_rows, pty_cols);
            }
        }
        if self.codex_terms.get(&key).is_none() && self.codex_spawn_error.is_none() {
            self.spawn_codex(&key);
        }
    }

    fn scroll_codex(&mut self, delta: isize) {
        let key = self.codex_binding_key();
        if let Some(term) = self.codex_terms.get(&key) {
            let max = term.scroll_len() as isize;
            let current = term.scroll_offset() as isize;
            let next = (current + delta).clamp(0, max);
            term.set_scroll(next as usize);
        }
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
                    selected: 0,
                });
                self.overlay_scroll = 0;
                self.spawn_bg(|client| BgResult::Inbox(client.tasks_inbox()));
            }
            "tasks" => {
                self.overlay = None;
                self.selected = 3; // Tasks
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
                        BgResult::TaskUpdated(client.tasks_update(&task_id, status, None))
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


    // -- overlays: selection + task detail -----------------------------------

    fn overlay_task_id(&self) -> Option<String> {
        match &self.overlay {
            Some(Overlay::TaskDetail { task_id, .. }) => Some(task_id.clone()),
            _ => None,
        }
    }

    fn move_inbox_selection(&mut self, delta: isize) {
        if let Some(Overlay::Inbox {
            state: OverlayState::Ready(items),
            selected,
        }) = &mut self.overlay
        {
            if items.is_empty() {
                *selected = 0;
                return;
            }
            let max = items.len() - 1;
            let next = (*selected as isize + delta).clamp(0, max as isize);
            *selected = next as usize;
        }
    }

    fn activate_inbox_selection(&mut self) {
        let task_id = match &self.overlay {
            Some(Overlay::Inbox {
                state: OverlayState::Ready(items),
                selected,
            }) => items.get(*selected).and_then(|item| item.task_id.clone()),
            _ => return,
        };
        match task_id {
            Some(task_id) => self.open_task_detail(task_id),
            None => {
                // Pure approvals carry no taskId: jump to the Approvals section.
                self.overlay = None;
                self.overlay_scroll = 0;
                self.selected = 5; // Approvals
            }
        }
    }

    fn open_task_detail(&mut self, task_id: String) {
        self.overlay = Some(Overlay::TaskDetail {
            state: Box::new(OverlayState::Loading),
            task_id: task_id.clone(),
        });
        self.overlay_scroll = 0;
        self.spawn_bg(move |client| {
            let brief = client.tasks_brief_task(&task_id);
            let messages = client.tasks_messages(&task_id);
            BgResult::TaskDetail(Box::new(TaskDetailPayload {
                task_id,
                brief,
                messages,
            }))
        });
    }

    /// `d`/`b` in the detail overlay: explicit operator control with a note;
    /// refreshes via the same path as every tasks.update from the UI.
    fn detail_mark(&mut self, task_id: &str, status: &'static str, note: &'static str) {
        self.composer_note = Some((format!("marking {task_id} {status}…"), false));
        let update_id = task_id.to_string();
        self.spawn_bg(move |client| {
            BgResult::TaskUpdated(client.tasks_update(&update_id, status, Some(note)))
        });
        // Re-load the detail view so title/status/brief reflect the transition.
        self.open_task_detail(task_id.to_string());
    }

    fn prefill_composer_for_task(&mut self, task_id: &str) {
        let text = match self.tasks.iter().position(|task| task.task_id == task_id) {
            Some(index) => format!("@{} ", index + 1),
            None => "@".to_string(),
        };
        self.overlay = None;
        self.overlay_scroll = 0;
        self.composer_text = text;
        self.composer_focused = true;
        self.composer_note = None;
    }

    // -- key handling --------------------------------------------------------

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        let code = key.code;
        let modifiers = key.modifiers;

        // Overlays consume everything until closed.
        if self.overlay.is_some() {
            let overlay_kind = match &self.overlay {
                Some(Overlay::Brief { .. }) => 0,
                Some(Overlay::Inbox { .. }) => 1,
                Some(Overlay::TaskDetail { .. }) => 2,
                None => unreachable!(),
            };
            match code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.overlay = None;
                    self.overlay_scroll = 0;
                }
                // Inbox: n/p (or arrows / j/k) move the selection, Enter acts.
                KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('p') if overlay_kind == 1 => {
                    self.move_inbox_selection(-1);
                }
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('n') if overlay_kind == 1 => {
                    self.move_inbox_selection(1);
                }
                KeyCode::Enter if overlay_kind == 1 => self.activate_inbox_selection(),
                // Task detail actions.
                KeyCode::Char('n') if overlay_kind == 2 => {
                    if let Some(task_id) = self.overlay_task_id() {
                        self.prefill_composer_for_task(&task_id);
                    }
                }
                KeyCode::Char('d') if overlay_kind == 2 => {
                    if let Some(task_id) = self.overlay_task_id() {
                        self.detail_mark(&task_id, "done", "marked done from TUI");
                    }
                }
                KeyCode::Char('b') if overlay_kind == 2 => {
                    if let Some(task_id) = self.overlay_task_id() {
                        self.detail_mark(&task_id, "blocked", "marked blocked from TUI");
                    }
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

        // Codex page: an embedded real terminal consuming every key except
        // the host chord (Ctrl+]) and the binding chords.
        if self.active_section() == CODEX_SECTION {
            let control = modifiers.contains(KeyModifiers::CONTROL);
            let shift = modifiers.contains(KeyModifiers::SHIFT);
            let binding_key = self.codex_binding_key();
            match code {
                // crossterm decodes 0x1D (Ctrl+]) as Char('5')+CONTROL on
                // unix terminals; accept both spellings.
                KeyCode::Char(']') | KeyCode::Char('5') if control => {
                    if self.mode == Mode::Simple {
                        return true; // quit the TUI
                    }
                    self.selected = 1; // back to Overview
                    return false;
                }
                KeyCode::Left if control => self.cycle_codex_binding(-1),
                KeyCode::Right if control => self.cycle_codex_binding(1),
                KeyCode::Up if control && shift => self.scroll_codex(3),
                KeyCode::Down if control && shift => self.scroll_codex(-3),
                KeyCode::Char('r') if control => self.respawn_codex(&binding_key.clone()),
                _ => {}
            }
            let intercepted = matches!(
                code,
                KeyCode::Char(']') | KeyCode::Char('5') if control
            ) || (control && matches!(code, KeyCode::Left | KeyCode::Right | KeyCode::Char('r')))
                || (control && shift && matches!(code, KeyCode::Up | KeyCode::Down));
            if !intercepted && let Some(bytes) = key_to_pty_bytes(&key) {
                if let Some(term) = self.codex_terms.get(&binding_key) {
                    let _ = term.write_input(&bytes);
                }
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
            KeyCode::Char('1'..='7') => {
                if let KeyCode::Char(number) = code {
                    self.selected = (number as usize) - ('1' as usize);
                }
            }
            // Focus-by-typing: printable characters that are not navigation
            // bindings start composing. '/', '@' and plain text all land here.
            KeyCode::Char(c)
                if !modifiers.contains(KeyModifiers::CONTROL)
                    && !modifiers.contains(KeyModifiers::ALT)
                    && !matches!(c, 'q' | 'j' | 'k' | 'h' | 'l' | 'n' | 'p' | '1'..='7') =>
            {
                self.composer_focused = true;
                self.composer_text.push(c);
                self.composer_note = None;
            }
            _ => {}
        }
        false
    }

    /// Geometry of the "✕ exit codex" button on the Codex page: the top-right
    /// border row of the pane (body starts below the 3-line header and the
    /// optional banner).
    fn codex_exit_button_rect(&self, width: u16) -> Rect {
        let button_width = 13;
        let y = if self.mode == Mode::Simple {
            0
        } else {
            3 + u16::from(
                self.connection == Connection::Connecting
                    || self.connection == Connection::Disconnected,
            )
        };
        Rect {
            x: width.saturating_sub(button_width + 1),
            y,
            width: button_width,
            height: 1,
        }
    }

    fn handle_mouse(&mut self, kind: MouseEventKind, column: u16, row: u16, width: u16) {
        // Codex page: the wheel drives the pty scrollback; a left click on the
        // exit button leaves the page.
        if self.mode == Mode::Simple || self.active_section() == CODEX_SECTION {
            match kind {
                MouseEventKind::ScrollUp | MouseEventKind::ScrollLeft => self.scroll_codex(3),
                MouseEventKind::ScrollDown | MouseEventKind::ScrollRight => self.scroll_codex(-3),
                MouseEventKind::Down(MouseButton::Left) => {
                    let button = self.codex_exit_button_rect(width);
                    let hit = column >= button.x
                        && column < button.x + button.width
                        && row == button.y;
                    if hit {
                        if self.mode == Mode::Simple {
                            // Simple layout has no other page: ✕ quits the TUI.
                            // We can't return true from here, so emulate quit
                            // via the quit flag below.
                            self.quit = true;
                        } else {
                            self.selected = 1; // back to Overview
                        }
                    }
                }
                _ => {}
            }
            return;
        }
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

/// Inbox items render grouped by kind; reorder once at apply time so the
/// overlay, the badge and Enter-to-act all share one ordering.
fn inbox_display_items(items: &[InboxItem]) -> Vec<InboxItem> {
    let mut ordered = Vec::with_capacity(items.len());
    for kind in INBOX_KIND_ORDER {
        ordered.extend(items.iter().filter(|item| item.kind == kind).cloned());
    }
    ordered.extend(
        items
            .iter()
            .filter(|item| !INBOX_KIND_ORDER.contains(&item.kind.as_str()))
            .cloned(),
    );
    ordered
}

fn build_task_detail(
    task_id: String,
    brief: Result<TaskBrief, RpcError>,
    messages: Result<Vec<TaskMessage>, RpcError>,
    project: Option<String>,
    task_title: Option<String>,
) -> TaskDetailView {
    let mut view = TaskDetailView {
        title: task_title.unwrap_or_else(|| task_id.clone()),
        status: String::new(),
        project: project.unwrap_or_else(|| "—".to_string()),
        summary: None,
        decisions: Vec::new(),
        blockers: Vec::new(),
        next_step: None,
        ceo: None,
        pending_messages: None,
        messages: Vec::new(),
        brief_error: None,
        messages_error: None,
    };
    match brief {
        Ok(brief) => {
            if !brief.title.is_empty() {
                view.title = brief.title;
            }
            view.status = brief.status;
            view.summary = brief.summary;
            view.decisions = brief.decisions;
            view.blockers = brief.blockers;
            view.next_step = brief.next_step;
            view.ceo = brief.ceo;
            view.pending_messages = brief.pending_messages;
        }
        Err(error) => view.brief_error = Some(brief_error(&error)),
    }
    match messages {
        Ok(mut items) => {
            items.reverse(); // daemon sends ts ASC; show newest first
            items.truncate(4);
            view.messages = items;
        }
        Err(error) => view.messages_error = Some(brief_error(&error)),
    }
    view
}

/// Greedy word-wrap; `width` is the usable inner width in columns.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    for raw in text.lines() {
        let mut current = String::new();
        for word in raw.split_whitespace() {
            if current.is_empty() {
                current.push_str(word);
            } else if current.chars().count() + 1 + word.chars().count() <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                out.push(std::mem::take(&mut current));
                current.push_str(word);
            }
        }
        if !current.is_empty() {
            out.push(current);
        } else if raw.trim().is_empty() {
            out.push(String::new());
        }
    }
    out
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

fn parse_mode() -> Result<Mode, String> {
    let mut mode = Mode::Simple;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--simple" | "-s" => mode = Mode::Simple,
            "--full" | "-f" => mode = Mode::Full,
            "--help" | "-h" => {
                println!("kbots-tui — minimal Codex chat terminal");
                println!();
                println!("USAGE: kbots-tui [--simple|--full]");
                println!();
                println!("  (default)   Simple layout: just the embedded Codex terminal,");
                println!("              a one-line header and a ✕ exit button (or Ctrl+]).");
                println!("  --full      The complete 7-section operator dashboard.");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other} (try --help)")),
        }
    }
    Ok(mode)
}

fn main() -> io::Result<()> {
    let mode = match parse_mode() {
        Ok(mode) => mode,
        Err(message) => {
            eprintln!("kbots-tui: {message}");
            std::process::exit(2);
        }
    };
    let mut app = App::new(mode);
    ratatui::run(|terminal| {
        let _mouse_capture = MouseCaptureGuard::enable()?;
        let mut needs_draw = true;
        loop {
            needs_draw |= app.apply_pending();
            let codex_active = app.active_section() == CODEX_SECTION;
            if codex_active {
                // Codex output is asynchronous; repaint on a short cadence.
                app.ensure_codex();
                if app.quit || app.quit_at.is_some_and(|at| Instant::now() >= at) {
                    break Ok(());
                }
                needs_draw = true;
            }
            if app.refresh_due() {
                app.start_refresh();
                needs_draw = true;
            }
            if needs_draw {
                terminal.draw(|frame| render(frame, &app))?;
                needs_draw = false;
            }

            let timeout = if app.refreshing || app.active_section() == CODEX_SECTION {
                Duration::from_millis(120) // animate the spinner / codex stream
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
                        if app.quit {
                            break Ok(());
                        }
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
    if app.mode == Mode::Simple {
        return render_simple(frame, app);
    }
    let area = frame.area();
    let banner = disconnected_banner(app);
    let codex_active = app.active_section() == CODEX_SECTION;
    let note_visible = !codex_active && app.composer_note.is_some();
    let note_height = 1;

    let mut constraints = vec![Constraint::Length(3)];
    if banner.is_some() {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Min(0)); // body
    if !codex_active {
        constraints.push(Constraint::Length(1)); // composer
    }
    if note_visible {
        constraints.push(Constraint::Length(note_height)); // action note
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
    if !codex_active {
        render_composer(frame, sections[index], app);
        index += 1;
    }
    if note_visible {
        render_note(frame, sections[index], app);
    }
    render_footer(frame, sections[sections.len() - 1], app);
    if app.overlay.is_some() {
        render_overlay(frame, area, app);
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

/// Simple layout (default): a one-line header and a fullscreen Codex
/// terminal with a one-click exit button. Nothing else.
fn render_simple(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);
    let live = match app.connection {
        Connection::Online => Span::styled(" LIVE ", Style::default().fg(Color::Black).bg(Color::Green).bold()),
        Connection::Connecting => Span::styled(" … ", Style::default().fg(Color::Black).bg(Color::Yellow).bold()),
        Connection::Disconnected => Span::styled(" OFFLINE ", Style::default().fg(Color::White).bg(Color::Red).bold()),
    };
    let line = Line::from(vec![
        Span::styled(" kbots ", Style::default().fg(Color::Black).bg(Color::Green).bold()),
        Span::raw(" "),
        Span::styled("CODEX", Style::default().fg(Color::White).bold()),
        Span::raw("  "),
        live,
        Span::raw("  · Ctrl+] to quit"),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(Color::Rgb(20, 24, 22))),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            " ✕ exit ",
            Style::default().fg(Color::Black).bg(Color::LightGreen).add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Right)
        .style(Style::default().bg(Color::Rgb(20, 24, 22))),
        chunks[0],
    );
    render_codex_section(frame, chunks[1], app);
}

fn render_body(frame: &mut Frame, area: Rect, app: &App) {
    if app.zoomed && app.active_section() == 1 {
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

    if sections[1].height < 8 && app.active_section() == 1 {
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
        0 => render_codex_section(frame, area, app),
        1 => render_overview(frame, area, app),
        2 => render_projects(frame, area, app),
        3 => render_tasks(frame, area, app),
        4 => render_agents(frame, area, app),
        5 => render_approvals(frame, area, app),
        6 => render_activity(frame, area, app),
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

/// Colour for a derived health level (`ok`, `waiting`, `warning`, `error`, …).
fn health_style(level: &str) -> Style {
    match level {
        "error" => Style::default().fg(Color::Red).bold(),
        "warning" => Style::default().fg(Color::Yellow),
        "ok" => Style::default().fg(Color::LightGreen),
        "waiting" => Style::default().fg(Color::LightBlue),
        _ => Style::default().fg(Color::DarkGray),
    }
}

fn health_glyph(level: &str) -> &'static str {
    match level {
        "error" => "✖",
        "warning" => "⚠",
        "ok" => "✔",
        _ => "·",
    }
}

fn agent_health(agent: &rpc::ManagedAgent) -> (&str, &str, &str) {
    let report = agent.report.as_ref();
    let level = report
        .and_then(|report| report.health.as_ref())
        .map(|health| health.level.as_str())
        .unwrap_or("unknown");
    let activity = report
        .and_then(|report| report.activity.as_ref())
        .map(|activity| activity.detail.as_str())
        .unwrap_or("");
    let message = report
        .and_then(|report| report.health.as_ref())
        .map(|health| health.message.as_str())
        .unwrap_or(agent.last_error.as_deref().unwrap_or(""));
    (level, activity, message)
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
        let (level, _, _) = agent_health(agent);
        lines.push(Line::from(vec![
            Span::styled(format!("{id:<10} "), Style::default().fg(Color::Cyan)),
            Span::raw(format!("{:<5} ", truncate_chars(&agent.role, 4))),
            Span::raw(format!("{:<10} ", truncate_chars(&agent.mode, 8))),
            Span::styled(
                format!("{:<10} ", truncate_chars(&agent.status, 8)),
                agent_status_style(&agent.status),
            ),
            Span::styled(format!("{} ", health_glyph(level)), health_style(level)),
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
            let (level, activity, message) = agent_health(agent);
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{id:<9} "), Style::default().fg(Color::Cyan)),
                Span::raw(format!("{:<6} ", truncate_chars(&agent.role, 5))),
                Span::raw(format!("{:<9} ", truncate_chars(&agent.mode, 8))),
                Span::styled(
                    format!("{:<9} ", truncate_chars(&agent.status, 8)),
                    agent_status_style(&agent.status),
                ),
                Span::styled(format!("{} ", health_glyph(level)), health_style(level)),
                Span::styled(managed_live_label(agent), Style::default().fg(Color::Gray)),
            ]));
            if !activity.is_empty() {
                lines.push(Line::from(vec![
                    Span::raw("     "),
                    Span::styled(
                        truncate_chars(activity, 60).to_string(),
                        Style::default().fg(Color::Gray),
                    ),
                    if let Some(task) = agent.report.as_ref().and_then(|report| report.task.as_ref()) {
                        Span::raw(format!(
                            " · {} {}",
                            task.task_id,
                            task.status
                        ))
                    } else {
                        Span::raw(String::new())
                    },
                ]));
            }
            if !message.is_empty() {
                lines.push(Line::from(vec![
                    Span::raw("     "),
                    Span::styled(
                        truncate_chars(message, 70).to_string(),
                        health_style(level),
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

// -- Codex -------------------------------------------------------------------

/// The Codex page embeds a real interactive Codex CLI in a pseudo-terminal
/// (see `codex_term.rs`). kbots forwards keys and renders the pty screen;
/// it never interprets Codex output.
fn render_codex_section(frame: &mut Frame, area: Rect, app: &App) {
    let key = app.codex_binding_key();
    let exited = app.codex_exited.iter().any(|entry| entry == &key);
    let mut title = format!(" Codex · {}", app.codex_binding_label());
    if app.codex_terms.order.len() > 1 {
        title.push_str(&format!(" · {} live", app.codex_terms.order.len()));
    }
    if exited {
        title.push_str(" · exited");
    }

    if let Some(error) = &app.codex_spawn_error {
        let message = format!(
            "{error}\n\n  Control is bound to the embedded Codex terminal (Ctrl+] to return to Overview).\n  Install the Codex CLI and make sure `codex` is on PATH, then press Ctrl+R."
        );
        frame.render_widget(
            Paragraph::new(message)
                .style(Style::default().fg(Color::Red))
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    match app.codex_terms.get(&key) {
        Some(term) => term.render(frame, area, &title, !exited),
        None => {
            frame.render_widget(
                Paragraph::new("starting codex…").style(Style::default().fg(Color::Gray)),
                area,
            );
        }
    }
    if app.mode == Mode::Simple {
        return;
    }
    // One-click exit: top-right corner of the pane border. Hit-tested in
    // handle_mouse with the same geometry.
    let button_width = 13;
    let button_x = area.x + area.width.saturating_sub(button_width + 1);
    frame.render_widget(
        Paragraph::new(Span::styled(
            " ✕ exit codex ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(Color::Rgb(20, 24, 22))),
        Rect {
            x: button_x,
            y: area.y,
            width: button_width,
            height: 1,
        },
    );
}

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
        let hint = "type a task, @N to message, /brief /inbox — Esc nav";
        Line::from(vec![
            Span::styled(" › ", Style::default().fg(Color::DarkGray)),
            Span::styled(hint, Style::default().fg(Color::DarkGray)),
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
    let left = if app.active_section() == CODEX_SECTION {
        if area.width < 70 {
            Line::from(vec![
                Span::styled("^] ", Style::default().fg(Color::LightGreen).bold()),
                Span::raw("dashboard · wheel scroll · ^R respawn"),
            ])
        } else {
            Line::from(vec![
                Span::styled("^] or click ✕ ", Style::default().fg(Color::LightGreen).bold()),
                Span::raw("dashboard · Ctrl+←/→ bind task · wheel scroll · Ctrl+R respawn · everything else goes to Codex"),
            ])
        }
    } else if area.width < 70 {
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

fn render_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(overlay) = &app.overlay else { return };
    let popup = overlay_area(area);
    frame.render_widget(Clear, popup);

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

    let mut selected_line: Option<usize> = None;
    let lines: Vec<Line> = match overlay {
        Overlay::Brief { state } => match state {
            OverlayState::Loading => vec![loading_line("loading brief", app.refreshing)],
            OverlayState::Failed(error) => failed_lines(error),
            OverlayState::Ready(brief) => brief_lines(brief),
        },
        Overlay::Inbox { state, selected } => match state {
            OverlayState::Loading => vec![loading_line("loading inbox", app.refreshing)],
            OverlayState::Failed(error) => failed_lines(error),
            OverlayState::Ready(items) => {
                let (lines, line) = inbox_lines(items, *selected);
                selected_line = Some(line);
                lines
            }
        },
        Overlay::TaskDetail { state, .. } => match state.as_ref() {
            OverlayState::Loading => vec![loading_line("loading task", app.refreshing)],
            OverlayState::Failed(error) => failed_lines(error),
            OverlayState::Ready(view) => detail_lines(view, inner.width as usize),
        },
    };

    let body = inner.height.max(1) as usize;
    // Reserve a hint row only when the content overflows.
    let content_rows = if lines.len() >= body {
        body.saturating_sub(1).max(1)
    } else {
        body
    };
    let mut start = 0usize;
    if lines.len() > content_rows {
        match selected_line {
            Some(selected) => {
                if selected >= content_rows {
                    start = selected + 1 - content_rows;
                }
            }
            None => {
                start = app.overlay_scroll.min(lines.len() - content_rows);
            }
        }
    }

    let mut all: Vec<Line> = lines.into_iter().skip(start).take(content_rows).collect();
    all.push(Line::from(Span::styled(
        overlay.hint(),
        Style::default().fg(Color::DarkGray),
    )));
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

/// Renders the display-ordered inbox items with group headers; returns the
/// lines plus the line index of the selected row so the window can follow it.
fn inbox_lines(items: &[InboxItem], selected: usize) -> (Vec<Line<'static>>, usize) {
    if items.is_empty() {
        return (
            vec![Line::from(Span::styled(
                "inbox zero — nothing needs you right now ✓",
                Style::default().fg(Color::LightGreen),
            ))],
            0,
        );
    }

    let mut lines = Vec::new();
    let mut selected_line = 0usize;
    let mut current_kind: Option<&str> = None;
    for (index, item) in items.iter().enumerate() {
        if current_kind != Some(item.kind.as_str()) {
            current_kind = Some(item.kind.as_str());
            let count = items.iter().filter(|other| other.kind == item.kind).count();
            let accent = inbox_kind_style(&item.kind);
            lines.push(Line::from(Span::styled(
                format!("{}  ({})", item.kind.to_uppercase(), count),
                Style::default().fg(accent).bold(),
            )));
        }
        let is_selected = index == selected;
        if is_selected {
            selected_line = lines.len();
        }
        let task = item.task_id.clone().unwrap_or_else(|| "—".to_string());
        let mut spans = vec![
            Span::styled(
                if is_selected { "› " } else { "  " },
                Style::default().fg(Color::LightGreen).bold(),
            ),
            Span::styled(
                format!("{:<10} ", truncate_chars(&task, 9)),
                Style::default().fg(Color::Cyan).bold(),
            ),
            Span::raw(truncate_chars(&item.summary(), 110)),
        ];
        if !item.task_id.is_some() {
            spans.push(Span::styled(
                "  approvals → Enter jumps to section 5",
                Style::default().fg(Color::DarkGray),
            ));
        }
        let line = Line::from(spans);
        if is_selected {
            lines.push(
                line.style(Style::default().bg(Color::Rgb(35, 49, 40)).add_modifier(Modifier::BOLD)),
            );
        } else {
            lines.push(line);
        }
    }
    (lines, selected_line)
}

/// Task detail view: brief sections and the last few messages, with
/// per-section inline errors instead of an all-or-nothing failure.
fn detail_lines(view: &TaskDetailView, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    let mut title_spans = vec![Span::styled(
        truncate_chars(&view.title, 64),
        Style::default().fg(Color::White).bold(),
    )];
    if !view.status.is_empty() {
        title_spans.push(Span::raw("   "));
        title_spans.push(Span::styled(
            view.status.clone(),
            task_status_style(&view.status),
        ));
    }
    lines.push(Line::from(title_spans));
    lines.push(Line::from(vec![
        Span::styled("project  ", Style::default().fg(Color::DarkGray)),
        Span::styled(view.project.clone(), Style::default().fg(Color::Magenta)),
    ]));

    // CEO line: id · mode · status, plus lastOutcome if the daemon reports it.
    match &view.ceo {
        Some(ceo) => {
            let mut spans = vec![
                Span::styled("CEO  ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    truncate_chars(&ceo.id, 20),
                    Style::default().fg(Color::Cyan).bold(),
                ),
            ];
            if let Some(mode) = &ceo.mode {
                spans.push(Span::raw(" · "));
                spans.push(Span::raw(mode.clone()));
            }
            if let Some(status) = &ceo.status {
                spans.push(Span::raw(" · "));
                spans.push(Span::styled(status.clone(), agent_status_style(status)));
            }
            lines.push(Line::from(spans));
            if let Some(outcome) = &ceo.last_outcome {
                let mut outcome_spans = vec![
                    Span::styled("     lastOutcome  ", Style::default().fg(Color::DarkGray)),
                    Span::styled(truncate_chars(outcome, 80), Style::default().fg(Color::White)),
                ];
                if let Some(at) = &ceo.last_outcome_at {
                    outcome_spans.push(Span::styled(
                        format!(" · {}", hhmmss(at)),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                lines.push(Line::from(outcome_spans));
            }
        }
        None => lines.push(Line::from(Span::styled(
            "CEO  none bound",
            Style::default().fg(Color::DarkGray),
        ))),
    }
    if let Some(count) = view.pending_messages {
        lines.push(Line::from(Span::styled(
            format!("queued messages to CEO: {count}"),
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines.push(Line::default());

    if let Some(error) = &view.brief_error {
        lines.push(Line::from(vec![
            Span::styled("brief unavailable: ", Style::default().fg(Color::Red).bold()),
            Span::styled(error.clone(), Style::default().fg(Color::Yellow)),
        ]));
    } else {
        if let Some(summary) = &view.summary {
            lines.push(Line::from(Span::styled(
                "SUMMARY",
                Style::default().fg(Color::Gray).bold(),
            )));
            for line in wrap_text(summary, width.saturating_sub(2)) {
                lines.push(Line::from(format!("  {line}")));
            }
            lines.push(Line::default());
        }
        push_detail_list(&mut lines, "DECISIONS", &view.decisions, Color::LightGreen, width);
        push_detail_list(&mut lines, "BLOCKERS", &view.blockers, Color::Red, width);
        if let Some(next) = &view.next_step {
            lines.push(Line::from(Span::styled(
                "NEXT STEP",
                Style::default().fg(Color::Yellow).bold(),
            )));
            for line in wrap_text(next, width.saturating_sub(2)) {
                lines.push(Line::from(format!("  {line}")));
            }
            lines.push(Line::default());
        }
        if view.summary.is_none()
            && view.decisions.is_empty()
            && view.blockers.is_empty()
            && view.next_step.is_none()
        {
            lines.push(Line::from(Span::styled(
                "no brief recorded yet",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::default());
        }
    }

    lines.push(Line::from(Span::styled(
        "MESSAGES · newest first",
        Style::default().fg(Color::Gray).bold(),
    )));
    if let Some(error) = &view.messages_error {
        lines.push(Line::from(vec![
            Span::styled("  messages unavailable: ", Style::default().fg(Color::Red).bold()),
            Span::styled(error.clone(), Style::default().fg(Color::Yellow)),
        ]));
    } else if view.messages.is_empty() {
        lines.push(Line::from(Span::styled(
            "  — no messages yet",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for message in &view.messages {
            let direction_style = match message.direction.as_str() {
                "user" => Style::default().fg(Color::Cyan),
                "system" => Style::default().fg(Color::Magenta),
                _ => Style::default().fg(Color::Gray),
            };
            let message_status_style = match message.status.as_str() {
                "delivered" => Style::default().fg(Color::LightGreen),
                "queued" => Style::default().fg(Color::Yellow),
                "canceled" => Style::default().fg(Color::DarkGray),
                _ => Style::default().fg(Color::Gray),
            };
            lines.push(Line::from(vec![
                Span::styled(
                    hhmmss(&message.ts).to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("{:<7} ", truncate_chars(&message.direction, 6)),
                    direction_style,
                ),
                Span::styled(
                    format!("{:<10} ", truncate_chars(&message.status, 9)),
                    message_status_style,
                ),
                Span::raw(truncate_chars(&message.text, 100)),
            ]));
        }
    }
    lines
}

fn push_detail_list(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    items: &[String],
    accent: Color,
    width: usize,
) {
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
            for (index, line) in wrap_text(item, width.saturating_sub(4)).into_iter().enumerate() {
                let prefix = if index == 0 { "  • " } else { "    " };
                lines.push(Line::from(format!("{prefix}{line}")));
            }
        }
    }
    lines.push(Line::default());
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
