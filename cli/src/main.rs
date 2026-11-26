use std::{
    env,
    error::Error,
    fmt, fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{self, Command},
};

use clap::{Parser, Subcommand};
use kbots_ipc as ipc;
use serde_json::{Value, json};

mod project_config;
mod render;
mod render_bots;

/// Walk up from `start` and return the nearest directory that contains
/// `.agents/bot/kbots.json` G�� the manifest `kbots project add` writes.
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

fn register_with_daemon(path: &Path, endpoints: &[ipc::Endpoint]) -> Result<Value, Box<dyn Error>> {
    let params = register_project(Some(path.to_path_buf()), None)?;
    let registered =
        ipc::call(endpoints, "projects.register", params).map_err(|e| Box::<dyn Error>::from(e))?;
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
    endpoints: &[ipc::Endpoint],
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
    register_with_daemon(&candidate, endpoints)?;
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

    /// Connect to a TCP daemon at HOST:PORT (or set KBOTS_HOST).
    #[arg(long, global = true, value_name = "HOST:PORT")]
    host: Option<String>,

    /// Token for TCP auth (or set KBOTS_TOKEN; defaults to the daemon's token file).
    #[arg(long, global = true)]
    token: Option<String>,

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
    /// Start, stop, or inspect a local daemon process (no service or admin rights).
    Daemon {
        #[command(subcommand)]
        command: DaemonCommands,
    },
    /// Remember a remote daemon address for all later commands.
    Connect {
        /// HOST:PORT of the remote daemon; omit with --local to clear it.
        host: Option<String>,
        /// Forget the saved remote daemon and use the local default again.
        #[arg(long)]
        local: bool,
    },
}

#[derive(Debug, Subcommand)]
enum DaemonCommands {
    /// Start the daemon in the background and write its pid to the state dir.
    Start,
    /// Stop the daemon recorded in the state dir (keeps all workflow state).
    Stop,
    /// Print whether the recorded daemon process is alive.
    Status,
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
        #[arg(value_parser = ["ceo", "git", "ops", "audit"])]
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
    if let Some(token) = cli.token {
        unsafe { env::set_var("KBOTS_TOKEN", token) };
    }
    match cli.command {
        Commands::Daemon { command } => daemon_command(command, cli.json),
        Commands::Connect { host, local } => {
            ipc::save_host(if local { None } else { host.as_deref() })?;
            let address = ipc::saved_host().unwrap_or_else(|| "local daemon".to_owned());
            emit("internal.connect", &json!({ "host": address }), cli.json);
            Ok(())
        }
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
            let endpoints = ipc::endpoints(cli.socket, cli.host);
            let (method, params) = rpc_call(command, &endpoints)?;
            let mut response = request_offering_register(&endpoints, method, params)?;
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
    endpoints: &[ipc::Endpoint],
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
                let project = project_for_command(project, endpoints)?;
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
                let project = project_for_command(project, endpoints)?;
                ("agents.list", json!({ "project": project }))
            }
            AgentCommands::Start {
                role,
                project,
                fresh,
            } => {
                let project = project_for_command(project, endpoints)?;
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
        Commands::Daemon { .. } | Commands::Connect { .. } => unreachable!("local command"),
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
        "botRoles": ["ceo", "git", "ops", "audit"],
    }))
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
    endpoints: &[ipc::Endpoint],
    method: &str,
    params: Value,
) -> Result<Value, Box<dyn Error>> {
    match ipc::call(endpoints, method, params.clone()) {
        Ok(response) => Ok(response),
        Err(ipc::RpcError::Method { code, message }) => {
            let daemon_error = DaemonRpcError {
                payload: json!({ "message": message }),
            };
            let Some(path) = unknown_project_directory(&daemon_error) else {
                return Err(Box::<dyn Error>::from(ipc::RpcError::Method {
                    code,
                    message,
                }));
            };
            if !confirm_register(&path)? {
                return Err(Box::<dyn Error>::from(ipc::RpcError::Method {
                    code,
                    message,
                }));
            }
            register_with_daemon(&path, endpoints)?;
            let mut retry = params;
            if let (Some(object), Some(path)) = (retry.as_object_mut(), path.to_str()) {
                object.insert("project".to_owned(), json!(path));
            }
            ipc::call(endpoints, method, retry).map_err(Into::into)
        }
        Err(other) => Err(Box::<dyn Error>::from(other)),
    }
}

/// administrator privileges: `daemon start` spawns `bun run daemon` detached.
fn daemon_pid_file() -> PathBuf {
    ipc::state_dir().join("daemon.pid")
}

fn read_pid() -> Option<u32> {
    fs::read_to_string(daemon_pid_file())
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0)
}

fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).contains(&format!(" {pid} ")))
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

fn daemon_command(command: DaemonCommands, json_output: bool) -> Result<(), Box<dyn Error>> {
    match command {
        DaemonCommands::Start => {
            if let Some(pid) = read_pid().filter(|pid| pid_alive(*pid)) {
                emit(
                    "daemon.start",
                    &json!({ "pid": pid, "alreadyRunning": true }),
                    json_output,
                );
                return Ok(());
            }
            let bun = env::var_os("KBOTS_BUN")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("bun"));
            let repo = ipc::repo_root().map(PathBuf::from).unwrap_or_else(|| {
                // Source checkouts work from their current directory; the
                // installer records the repo root for installed shims.
                env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
            });
            fs::create_dir_all(ipc::state_dir())?;
            // One log file, truncated per start; enough to diagnose a failed
            // background start without a service manager.
            let log = fs::File::create(ipc::state_dir().join("daemon.log"))?;
            let log_err = log.try_clone()?;
            let mut child = Command::new(bun);
            child
                .args(["run", "daemon"])
                .current_dir(&repo)
                .stdout(std::process::Stdio::from(log))
                .stderr(std::process::Stdio::from(log_err));
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const DETACHED_PROCESS: u32 = 0x0000_0008;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                child.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
            }
            let child = child
                .spawn()
                .map_err(|error| format!("cannot start daemon with bun: {error}"))?;
            let pid = child.id();
            fs::write(daemon_pid_file(), format!("{pid}\n"))?;
            emit(
                "daemon.start",
                &json!({ "pid": pid, "started": true }),
                json_output,
            );
        }
        DaemonCommands::Stop => {
            let pid =
                read_pid().ok_or("no recorded daemon process (run `kbots daemon start` first)")?;
            if !pid_alive(pid) {
                let _ = fs::remove_file(daemon_pid_file());
                emit(
                    "daemon.stop",
                    &json!({ "pid": pid, "wasRunning": false }),
                    json_output,
                );
                return Ok(());
            }
            #[cfg(windows)]
            let stopped = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            #[cfg(not(windows))]
            let stopped = Command::new("kill")
                .arg(pid.to_string())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !stopped {
                return Err(format!("could not stop daemon process {pid}").into());
            }
            let _ = fs::remove_file(daemon_pid_file());
            emit(
                "daemon.stop",
                &json!({ "pid": pid, "stopped": true }),
                json_output,
            );
        }
        DaemonCommands::Status => {
            let pid = read_pid();
            let running = pid.map(pid_alive).unwrap_or(false);
            emit(
                "daemon.status",
                &json!({ "pid": pid, "running": running }),
                json_output,
            );
        }
    }
    Ok(())
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
                "message": "role must be one of ceo, git, ops, audit"
            }),
        };
        assert_eq!(
            error.to_string(),
            "role must be one of ceo, git, ops, audit"
        );
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
