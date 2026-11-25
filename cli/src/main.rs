use std::{
    env,
    error::Error,
    fmt, fs,
    io::{self, BufRead, BufReader, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{self, Command},
    time::Duration,
};

use clap::{Parser, Subcommand};
use serde_json::{Value, json};

mod project_config;
mod render;
mod render_bots;

/// Walk up from `start` and return the nearest directory that contains
/// `.agents/bot/kbots.json` — the manifest `kbots project add` writes.
/// A bare `.agents/bot` tree (other tools, hand-written roles) is not a
/// kbots project and must not be sent to the daemon as a scope.
fn enclosing_project_path(start: PathBuf) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        if dir.join(".agents/bot/kbots.json").is_file() {
            return fs::canonicalize(&dir).ok();
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Project that owns the current directory, when one exists.
fn current_project_path() -> Option<PathBuf> {
    enclosing_project_path(env::current_dir().ok()?)
}

/// Nearest ancestor that has `.agents/bot/` but no `kbots.json`.
/// Returns None when `start` is already inside a kbots project.
fn unregistered_project_root(start: PathBuf) -> Option<PathBuf> {
    if enclosing_project_path(start.clone()).is_some() {
        return None;
    }
    let mut dir = start;
    loop {
        if dir.join(".agents/bot").is_dir() {
            return fs::canonicalize(&dir).ok();
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn answer_is_yes(line: &str) -> bool {
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Ask before writing the scaffold and registering `path`. A non-interactive
/// stdin (piped scripts) skips the prompt and does not add the project.
fn confirm_register(path: &Path) -> Result<bool, Box<dyn Error>> {
    if !io::stdin().is_terminal() {
        return Ok(false);
    }
    eprint!("Add {} to kbots? [y/N] ", path.display());
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(answer_is_yes(&line))
}

fn register_with_daemon(path: &Path, sockets: &[PathBuf]) -> Result<Value, Box<dyn Error>> {
    let params = register_project(Some(path.to_path_buf()), None)?;
    let registered = request_with_fallback(sockets, "projects.register", params)?;
    let id = registered
        .get("projectId")
        .and_then(Value::as_str)
        .unwrap_or("project");
    let name = registered
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("project");
    eprintln!("Registered {name} ({id}).");
    Ok(registered)
}

/// Resolve `--project`. An omitted value uses the enclosing kbots project.
/// A directory that only has `.agents/bot/` is offered for registration when
/// stdin is a terminal; declining leaves the command unscoped.
fn project_for_command(
    explicit: Option<String>,
    sockets: &[PathBuf],
) -> Result<Option<String>, Box<dyn Error>> {
    if let Some(project) = explicit.filter(|value| !value.trim().is_empty()) {
        return Ok(Some(project));
    }
    if let Some(path) = current_project_path() {
        return Ok(Some(path.to_string_lossy().into_owned()));
    }
    let Some(candidate) = unregistered_project_root(env::current_dir()?) else {
        return Ok(None);
    };
    if !confirm_register(&candidate)? {
        return Ok(None);
    }
    register_with_daemon(&candidate, sockets)?;
    Ok(Some(candidate.to_string_lossy().into_owned()))
}

#[derive(Debug, Parser)]
#[command(
    name = "kbots",
    version,
    about = "Automation CLI for the kbots coding workflow"
)]
struct Cli {
    /// Override the daemon's Unix socket path (or set KBOTS_SOCKET).
    #[arg(long, global = true, value_name = "PATH")]
    socket: Option<PathBuf>,

    /// Print JSON output even when stdout is a terminal.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Read the current daemon and workflow summary as JSON.
    Status,
    /// Manage workflow tasks.
    Tasks {
        #[command(subcommand)]
        command: TaskCommands,
    },
    /// List registered coding agents.
    #[command(visible_alias = "ag")]
    Agents {
        #[command(subcommand)]
        command: AgentCommands,
    },
    /// Start the enabled OpenCode bot sessions for a project.
    Sessions {
        #[command(subcommand)]
        command: SessionCommands,
    },
    /// Register and inspect local projects.
    #[command(visible_alias = "prj")]
    Project {
        #[command(subcommand)]
        command: ProjectCommands,
    },
    /// Show the daemon activity log, newest first.
    Events {
        /// Maximum number of events (1-500, default 100).
        #[arg(long)]
        limit: Option<usize>,
    },
    /// List approvals (pending by default).
    #[command(visible_alias = "appr")]
    Approvals {
        /// Filter: pending (default) or approved.
        #[arg(long)]
        status: Option<String>,
    },
    /// Approve a pending protected operation.
    Approve {
        /// Approval identifier.
        approval_id: String,
    },
    /// Open the Slint desktop control center.
    Gui,
    /// Open the Ratatui terminal control center (simple Codex layout by default).
    Tui {
        /// Open the full 7-section operator dashboard instead of the minimal Codex-only layout.
        #[arg(long)]
        full: bool,
    },
}

#[derive(Debug, Subcommand)]
enum TaskCommands {
    /// Submit a task to the CEO planner.
    Add {
        /// Task description.
        title: String,
        /// Project identifier or path.
        #[arg(long, short = 'p')]
        project: Option<String>,
        /// Expanded goal given to the CEO's first prompt.
        #[arg(long)]
        goal: Option<String>,
        /// Extra context; shown to the CEO.
        #[arg(long)]
        detail: Option<String>,
    },
    /// List tasks, optionally filtered by project and status.
    #[command(visible_alias = "ls")]
    List {
        #[arg(long, short = 'p')]
        project: Option<String>,
        /// Only tasks in this status (queued, researching, planning, delegating, running, review, blocked, done, failed, canceled).
        #[arg(long)]
        status: Option<String>,
    },
    /// Show one task.
    Show { task_id: String },
    /// Cancel one task.
    Cancel { task_id: String },
    /// Send a message to a task.
    Message { task_id: String, text: String },
    /// List messages for a task, newest last.
    Messages { task_id: String },
    /// Print the global brief, or one task's brief when TASK_ID is given.
    Brief { task_id: Option<String> },
    /// Show items that need action, grouped by kind.
    Inbox,
    /// Update a task's status (review, blocked, done, failed, or canceled).
    Update {
        task_id: String,
        /// New status; validated by the daemon.
        #[arg(long)]
        status: String,
        /// Note recorded with the status change.
        #[arg(long)]
        note: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum AgentCommands {
    /// List agent roles, active runs, and session identifiers.
    #[command(visible_alias = "ls")]
    List {
        /// Only show this project (id, name, or path).
        #[arg(long, short = 'p')]
        project: Option<String>,
        /// Also list finished (stopped) agents; hidden by default.
        #[arg(long)]
        all: bool,
    },
    /// Start (or resume) a bot agent for a registered project.
    Start {
        /// Bot role to start.
        #[arg(value_parser = ["ceo", "git", "ops"])]
        role: String,
        /// Project id, name, or path (defaults to the current directory).
        #[arg(long, short = 'p')]
        project: Option<String>,
        /// Start a fresh session instead of resuming the existing one.
        #[arg(long)]
        fresh: bool,
    },
    /// Show one agent and probe its runtime.
    Show { bot_id: String },
    /// Print an agent's session history and its live terminal screen.
    Logs {
        bot_id: String,
        /// How many messages to fetch (1-200, default 30).
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Stop one agent (keeps its OpenCode session and history).
    Stop { bot_id: String },
    /// Send a prompt to one agent.
    Prompt { bot_id: String, text: String },
    /// Interrupt one agent's current run (keeps session and terminal).
    Interrupt { bot_id: String },
}

#[derive(Debug, Subcommand)]
enum SessionCommands {
    /// Create idle CEO, GIT, and OPS sessions defined by the project manifest.
    Start {
        /// Existing project directory (defaults to the current directory).
        #[arg(long)]
        directory: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum ProjectCommands {
    /// Register a local project (defaults to the current directory).
    Add {
        /// Existing project directory.
        path: Option<PathBuf>,
        /// Display name; defaults to the directory name.
        #[arg(long)]
        name: Option<String>,
    },
    /// List registered projects.
    #[command(visible_alias = "ls")]
    List,
    /// Show a registered project by ID.
    Show { project_id: String },
    /// Manage the file-change watcher for a project (defaults to listing).
    Watch {
        /// Project id, name, or path (omit to list watchers).
        #[arg(long, short = 'p')]
        project: Option<String>,
        /// Disable the watcher instead of enabling it.
        #[arg(long)]
        off: bool,
        /// Poll interval in seconds.
        #[arg(long)]
        interval: Option<u64>,
        /// Quiet period before prompting, in seconds.
        #[arg(long)]
        debounce: Option<u64>,
        /// Suppress delivery for this many seconds after a prompt.
        #[arg(long)]
        cooldown: Option<u64>,
        /// Cap how many paths are included in the prompt delta.
        #[arg(long)]
        max_paths: Option<u64>,
    },
    /// Create or regenerate per-bot OpenCode configs from .agents/bot/kbots.json.
    Configure {
        /// Existing project directory (defaults to the current directory).
        path: Option<PathBuf>,
        /// Replace existing per-bot opencode.jsonc files from the manifest.
        #[arg(long)]
        overwrite: bool,
    },
    /// Sync per-role AGENTS.md starters from the embedded templates.
    Sync {
        /// Existing project directory (defaults to the current directory).
        path: Option<PathBuf>,
        /// Replace customized AGENTS.md files too; each is backed up first (AGENTS.md.bak[.N]).
        #[arg(long)]
        force: bool,
        /// Report whether the repo is up to date; write nothing.
        #[arg(long)]
        check: bool,
    },
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("kbots: {error}");
        process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    match cli.command {
        Commands::Gui => run_ui("KBOTS_GUI_BIN", "kbots-gui", None),
        Commands::Tui { full } => run_ui("KBOTS_TUI_BIN", "kbots-tui", full.then_some("--full")),
        Commands::Project {
            command: ProjectCommands::Configure { path, overwrite },
        } => {
            let result = project_config::configure_project(path, overwrite)?;
            emit("internal.project.configure", &result, cli.json);
            Ok(())
        }
        Commands::Project {
            command: ProjectCommands::Sync { path, force, check },
        } => {
            let result = project_config::sync_project(path, force, check)?;
            emit("internal.project.sync", &result, cli.json);
            Ok(())
        }
        Commands::Sessions {
            command: SessionCommands::Start { directory },
        } => run_sessions_start(directory, cli.json),
        command => {
            let candidates = socket_candidates(cli.socket);
            let (method, params) = rpc_call(command, &candidates)?;
            let mut response = request_offering_register(&candidates, method, params)?;
            if method == "projects.list" {
                project_config::enrich_projects_list(&mut response);
            }
            emit(method, &response, cli.json);
            Ok(())
        }
    }
}

fn emit(method: &str, value: &Value, force_json: bool) {
    if force_json || !io::stdout().is_terminal() {
        print_json(value);
        return;
    }
    // `--all` is the render hint to expand collapsed finished groups (agents.list)
    let show_finished = std::env::args().any(|arg| arg == "--all");
    match render::render(method, value, show_finished) {
        Some(text) => println!("{text}"),
        None => print_json(value),
    }
}

fn print_json(value: &Value) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(_) => println!("{value}"),
    }
}

fn rpc_call(
    command: Commands,
    sockets: &[PathBuf],
) -> Result<(&'static str, Value), Box<dyn Error>> {
    let request = match command {
        Commands::Status => ("system.status", json!({})),
        Commands::Events { limit } => ("events.list", json!({ "limit": limit })),
        Commands::Approvals { status } => ("approvals.list", json!({ "status": status })),
        Commands::Tasks { command } => match command {
            TaskCommands::Add {
                title,
                project,
                goal,
                detail,
            } => {
                let mut params = json!({ "title": title, "project": project });
                if let Some(goal) = goal.filter(|text| !text.trim().is_empty()) {
                    params["goal"] = json!(goal);
                }
                if let Some(detail) = detail.filter(|text| !text.trim().is_empty()) {
                    params["detail"] = json!(detail);
                }
                ("tasks.create", params)
            }
            TaskCommands::List { project, status } => {
                let project = project_for_command(project, sockets)?;
                (
                    "tasks.list",
                    json!({ "project": project, "status": status }),
                )
            }
            TaskCommands::Show { task_id } => ("tasks.get", json!({ "taskId": task_id })),
            TaskCommands::Cancel { task_id } => ("tasks.cancel", json!({ "taskId": task_id })),
            TaskCommands::Message { task_id, text } => {
                ("tasks.message", json!({ "taskId": task_id, "text": text }))
            }
            TaskCommands::Messages { task_id } => ("tasks.messages", json!({ "taskId": task_id })),
            TaskCommands::Brief { task_id } => match task_id {
                Some(task_id) => ("tasks.brief", json!({ "taskId": task_id })),
                None => ("tasks.brief", json!({})),
            },
            TaskCommands::Inbox => ("tasks.inbox", json!({})),
            TaskCommands::Update {
                task_id,
                status,
                note,
            } => {
                let mut params = json!({ "taskId": task_id, "status": status });
                if let Some(note) = note.filter(|text| !text.trim().is_empty()) {
                    params["note"] = json!(note);
                }
                ("tasks.update", params)
            }
        },
        Commands::Agents { command } => match command {
            AgentCommands::List { project, .. } => {
                let project = project_for_command(project, sockets)?;
                ("agents.list", json!({ "project": project }))
            }
            AgentCommands::Start {
                role,
                project,
                fresh,
            } => {
                let project = project_for_command(project, sockets)?;
                (
                    "agents.start",
                    json!({ "role": role, "project": project, "fresh": fresh }),
                )
            }
            AgentCommands::Show { bot_id } => ("agents.show", json!({ "botId": bot_id })),
            AgentCommands::Logs { bot_id, limit } => {
                ("agents.logs", json!({ "botId": bot_id, "limit": limit }))
            }
            AgentCommands::Stop { bot_id } => ("agents.stop", json!({ "botId": bot_id })),
            AgentCommands::Prompt { bot_id, text } => {
                ("agents.prompt", json!({ "botId": bot_id, "text": text }))
            }
            AgentCommands::Interrupt { bot_id } => ("agents.interrupt", json!({ "botId": bot_id })),
        },
        Commands::Sessions { .. } => unreachable!("session commands are handled before RPC"),
        Commands::Project { command } => match command {
            ProjectCommands::Add { path, name } => {
                ("projects.register", register_project(path, name)?)
            }
            ProjectCommands::List => ("projects.list", json!({})),
            ProjectCommands::Show { project_id } => {
                ("projects.get", json!({ "projectId": project_id }))
            }
            ProjectCommands::Watch {
                project,
                off,
                interval,
                debounce,
                cooldown,
                max_paths,
            } => match project {
                Some(project) => (
                    "watchers.set",
                    json!({
                        "project": project,
                        "enabled": !off,
                        "intervalSeconds": interval,
                        "debounceSeconds": debounce,
                        "cooldownSeconds": cooldown,
                        "maxPaths": max_paths,
                    }),
                ),
                None => ("watchers.list", json!({})),
            },
            ProjectCommands::Configure { .. } => unreachable!("handled before RPC"),
            ProjectCommands::Sync { .. } => unreachable!("handled before RPC"),
        },
        Commands::Approve { approval_id } => {
            ("approvals.approve", json!({ "approvalId": approval_id }))
        }
        Commands::Gui | Commands::Tui { .. } => unreachable!("UI commands are handled before RPC"),
    };
    Ok(request)
}

fn run_sessions_start(directory: Option<PathBuf>, force_json: bool) -> Result<(), Box<dyn Error>> {
    let source_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let script = source_root.join("src/backend/cli/start-sessions.ts");
    if !script.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "kbots session manager script not found: {}",
                script.display()
            ),
        )
        .into());
    }

    let mut command = Command::new("bun");
    command.arg("run").arg(script);
    if let Some(directory) = directory {
        command.arg("--directory").arg(directory);
    }
    let output = command.output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        eprint!("{stderr}");
    }
    if !output.status.success() {
        return Err(
            io::Error::other(format!("session startup failed with {}", output.status)).into(),
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if force_json || !io::stdout().is_terminal() {
        print!("{stdout}");
        if !stdout.ends_with('\n') {
            println!();
        }
        return Ok(());
    }
    match serde_json::from_str::<Value>(stdout.trim())
        .ok()
        .and_then(|value| render::render("internal.sessions.start", &value, false))
    {
        Some(text) => println!("{text}"),
        None => print!("{stdout}"),
    }
    Ok(())
}

fn register_project(path: Option<PathBuf>, name: Option<String>) -> Result<Value, Box<dyn Error>> {
    let requested_path = path.unwrap_or(env::current_dir()?);
    let project_path = fs::canonicalize(&requested_path)?;
    if !project_path.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "project path is not a directory: {}",
                project_path.display()
            ),
        )
        .into());
    }

    let project_name = match name {
        Some(name) if !name.trim().is_empty() => name.trim().to_owned(),
        Some(_) => {
            return Err(
                io::Error::new(io::ErrorKind::InvalidInput, "--name cannot be empty").into(),
            );
        }
        None => project_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_else(|| project_path.to_str().unwrap_or("project"))
            .to_owned(),
    };
    let path = project_path.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "project path cannot be represented as UTF-8",
        )
    })?;

    project_config::ensure_project_bot_scaffold(&project_path)?;

    Ok(json!({
        "path": path,
        "name": project_name,
        "botRoles": ["ceo", "git", "ops"],
    }))
}

/// Socket candidates in the contract's discovery order. An explicit
/// `--socket` or `KBOTS_SOCKET` means exactly that one path; otherwise the
/// clients fall back to the next candidate when a connect fails (a sandbox
/// may deny /run/user/* while $HOME works).
fn socket_candidates(override_path: Option<PathBuf>) -> Vec<PathBuf> {
    if let Some(path) = override_path {
        return vec![path];
    }
    if let Some(path) = env::var_os("KBOTS_SOCKET") {
        return vec![PathBuf::from(path)];
    }
    let mut candidates = Vec::new();
    if let Some(runtime_dir) = env::var_os("XDG_RUNTIME_DIR") {
        candidates.push(PathBuf::from(runtime_dir).join("kbots.sock"));
    }
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/run/kbots.sock");
    if !candidates.contains(&home) {
        candidates.push(home);
    }
    candidates
}

/// JSON-RPC `error` object from a daemon that accepted the connection.
/// Distinct from a socket that could not be reached, so fallback does not
/// report an application error as "start the daemon".
#[derive(Debug)]
struct DaemonRpcError {
    payload: Value,
}

impl fmt::Display for DaemonRpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = self
            .payload
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("");
        const UNKNOWN_PROJECT: &str = "unknown project:";
        if let Some(path) = message.strip_prefix(UNKNOWN_PROJECT) {
            let path = path.trim();
            return write!(
                f,
                "unknown project: {path}. Register it with `kb project add {path}`"
            );
        }
        if !message.is_empty() {
            return write!(f, "{message}");
        }
        write!(f, "daemon returned an error: {}", self.payload)
    }
}

impl Error for DaemonRpcError {}

fn request_with_fallback(
    candidates: &[PathBuf],
    method: &str,
    params: Value,
) -> Result<Value, Box<dyn Error>> {
    let mut errors = Vec::with_capacity(candidates.len());
    for socket in candidates {
        match request(socket, method, params.clone()) {
            Ok(response) => return Ok(response),
            Err(error) if error.downcast_ref::<DaemonRpcError>().is_some() => return Err(error),
            Err(error) => errors.push(format!("{}: {error}", socket.display())),
        }
    }
    Err(format!(
        "cannot connect to daemon socket (tried: {}); start the kbots daemon or set KBOTS_SOCKET",
        errors.join("; ")
    )
    .into())
}

fn unknown_project_directory(daemon: &DaemonRpcError) -> Option<PathBuf> {
    let message = daemon.payload.get("message")?.as_str()?;
    let path = message.strip_prefix("unknown project:")?.trim();
    let path = PathBuf::from(path);
    if !path.is_dir() {
        return None;
    }
    fs::canonicalize(&path).ok().or(Some(path))
}

/// When the daemon rejects a directory it has never registered, ask to add
/// it and retry the same call once. Declining, or a non-interactive stdin,
/// keeps the original error.
fn request_offering_register(
    candidates: &[PathBuf],
    method: &str,
    params: Value,
) -> Result<Value, Box<dyn Error>> {
    match request_with_fallback(candidates, method, params.clone()) {
        Ok(response) => Ok(response),
        Err(error) => {
            let Some(path) = error
                .downcast_ref::<DaemonRpcError>()
                .and_then(unknown_project_directory)
            else {
                return Err(error);
            };
            if !confirm_register(&path)? {
                return Err(error);
            }
            register_with_daemon(&path, candidates)?;
            let mut retry = params;
            if let (Some(object), Some(path)) = (retry.as_object_mut(), path.to_str()) {
                object.insert("project".to_owned(), json!(path));
            }
            request_with_fallback(candidates, method, retry)
        }
    }
}

#[cfg(unix)]
fn request(socket: &std::path::Path, method: &str, params: Value) -> Result<Value, Box<dyn Error>> {
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(socket).map_err(|error| io::Error::from(error))?;
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;

    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    writeln!(stream, "{request}")?;
    let mut response_line = String::new();
    BufReader::new(stream).read_line(&mut response_line)?;
    if response_line.is_empty() {
        return Err("daemon closed the socket without sending a response".into());
    }

    let response: Value = serde_json::from_str(&response_line)?;
    if let Some(error) = response.get("error") {
        return Err(DaemonRpcError {
            payload: error.clone(),
        }
        .into());
    }
    if response.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || response.get("id") != Some(&json!(1))
    {
        return Err("daemon returned a response with an invalid JSON-RPC version or id".into());
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| "daemon response did not include a result".into())
}

#[cfg(not(unix))]
fn request(
    _socket: &std::path::Path,
    _method: &str,
    _params: Value,
) -> Result<Value, Box<dyn Error>> {
    Err("the kbots daemon currently uses Unix domain sockets and requires a Unix host".into())
}

fn run_ui(env_name: &str, binary_name: &str, arg: Option<&str>) -> Result<(), Box<dyn Error>> {
    let executable = env::var_os(env_name).map(PathBuf::from).unwrap_or_else(|| {
        env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join(binary_name)))
            .filter(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from(binary_name))
    });
    let mut command = Command::new(&executable);
    if let Some(arg) = arg {
        command.arg(arg);
    }
    let status = command.status().map_err(|error| {
        io::Error::other(format!(
            "cannot start {} at {}: {error}",
            binary_name,
            executable.display()
        ))
    })?;
    process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_project_error_names_project_add() {
        let error = DaemonRpcError {
            payload: json!({
                "code": -32602,
                "message": "unknown project: /tmp/film-production"
            }),
        };
        assert_eq!(
            error.to_string(),
            "unknown project: /tmp/film-production. Register it with `kb project add /tmp/film-production`"
        );
    }

    #[test]
    fn other_daemon_errors_keep_the_message() {
        let error = DaemonRpcError {
            payload: json!({
                "code": -32602,
                "message": "role must be one of ceo, git, ops"
            }),
        };
        assert_eq!(error.to_string(), "role must be one of ceo, git, ops");
    }

    #[test]
    fn enclosing_project_requires_kbots_manifest() {
        let root = std::env::temp_dir().join(format!("kbots-scope-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        let bare = root.join("bare/.agents/bot/ceo");
        fs::create_dir_all(&bare).unwrap();
        assert!(enclosing_project_path(bare).is_none());

        let managed = root.join("managed");
        let manifest_dir = managed.join(".agents/bot");
        fs::create_dir_all(&manifest_dir).unwrap();
        fs::write(manifest_dir.join("kbots.json"), "{}").unwrap();

        // A nearer `.agents/bot` without a manifest must not hide the real project.
        let nested = managed.join("vendor/.agents/bot/ceo");
        fs::create_dir_all(&nested).unwrap();
        let found = enclosing_project_path(nested).unwrap();
        assert_eq!(found, fs::canonicalize(&managed).unwrap());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn approval_accepts_only_yes() {
        assert!(answer_is_yes("y"));
        assert!(answer_is_yes("YES"));
        assert!(answer_is_yes("  yes \n"));
        assert!(!answer_is_yes(""));
        assert!(!answer_is_yes("n"));
        assert!(!answer_is_yes("no"));
    }

    #[test]
    fn unregistered_root_is_the_agents_bot_directory() {
        let root = std::env::temp_dir().join(format!("kbots-offer-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        let bare = root.join("bare");
        fs::create_dir_all(bare.join(".agents/bot/ceo")).unwrap();
        let nested = bare.join("src");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(
            unregistered_project_root(nested).unwrap(),
            fs::canonicalize(&bare).unwrap()
        );

        let managed = root.join("managed");
        fs::create_dir_all(managed.join(".agents/bot")).unwrap();
        fs::write(managed.join(".agents/bot/kbots.json"), "{}").unwrap();
        assert!(unregistered_project_root(managed.join("src")).is_none());

        let plain = root.join("plain/src");
        fs::create_dir_all(&plain).unwrap();
        assert!(unregistered_project_root(plain).is_none());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_project_directory_requires_a_real_directory() {
        let missing = DaemonRpcError {
            payload: json!({"code": -32602, "message": "unknown project: /no/such/kbots-project"}),
        };
        assert!(unknown_project_directory(&missing).is_none());

        let named = DaemonRpcError {
            payload: json!({"code": -32602, "message": "unknown project: PRJ-99"}),
        };
        assert!(unknown_project_directory(&named).is_none());

        let root = std::env::temp_dir().join(format!("kbots-unknown-dir-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let message = format!("unknown project: {}", root.display());
        let existing = DaemonRpcError {
            payload: json!({"code": -32602, "message": message}),
        };
        assert_eq!(
            unknown_project_directory(&existing).unwrap(),
            fs::canonicalize(&root).unwrap()
        );
        let _ = fs::remove_dir_all(&root);
    }
}
