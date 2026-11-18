use std::{
    env,
    error::Error,
    fs,
    io::{self, BufRead, BufReader, IsTerminal, Write},
    path::PathBuf,
    process::{self, Command},
    time::Duration,
};

use clap::{Parser, Subcommand};
use serde_json::{Value, json};

mod project_config;
mod render;

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
    /// Approve a pending protected operation.
    Approve {
        /// Approval identifier.
        approval_id: String,
    },
    /// Open the Slint desktop control center.
    Gui,
    /// Open the Ratatui terminal control center.
    Tui,
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
    /// List tasks, optionally filtered by project.
    #[command(visible_alias = "ls")]
    List {
        #[arg(long, short = 'p')]
        project: Option<String>,
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
    },
    /// Start (or resume) a bot agent for a registered project.
    Start {
        /// Bot role to start.
        #[arg(value_parser = ["ceo", "git", "ops"])]
        role: String,
        /// Runtime mode.
        #[arg(long, value_enum, default_value_t = AgentMode::Terminal)]
        mode: AgentMode,
        /// Project id, name, or path (defaults to the current directory).
        #[arg(long, short = 'p')]
        project: Option<String>,
        /// Start a fresh session instead of resuming the existing one.
        #[arg(long)]
        fresh: bool,
    },
    /// Show one agent and probe its runtime.
    Show { bot_id: String },
    /// Attach an hc terminal to a running headless agent (same session).
    Attach { bot_id: String },
    /// Stop one agent (keeps its OpenCode session and history).
    Stop { bot_id: String },
    /// Send a prompt to one agent.
    Prompt { bot_id: String, text: String },
    /// Interrupt one agent's current run (keeps session and terminal).
    Interrupt { bot_id: String },
}

#[derive(Debug, Clone, clap::ValueEnum)]
enum AgentMode {
    Headless,
    Terminal,
}

impl AgentMode {
    fn as_str(&self) -> &'static str {
        match self {
            AgentMode::Headless => "headless",
            AgentMode::Terminal => "terminal",
        }
    }
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
        Commands::Gui => run_ui("KBOTS_GUI_BIN", "kbots-gui"),
        Commands::Tui => run_ui("KBOTS_TUI_BIN", "kbots-tui"),
        Commands::Project {
            command: ProjectCommands::Configure { path, overwrite },
        } => {
            let result = project_config::configure_project(path, overwrite)?;
            emit("internal.project.configure", &result, cli.json);
            Ok(())
        }
        Commands::Project {
            command: ProjectCommands::Sync { path, force },
        } => {
            let result = project_config::sync_project(path, force)?;
            emit("internal.project.sync", &result, cli.json);
            Ok(())
        }
        Commands::Sessions {
            command: SessionCommands::Start { directory },
        } => run_sessions_start(directory, cli.json),
        command => {
            let (method, params) = rpc_call(command)?;
            let socket = socket_path(cli.socket);
            let response = request(&socket, method, params)?;
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
    match render::render(method, value) {
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

fn rpc_call(command: Commands) -> Result<(&'static str, Value), Box<dyn Error>> {
    let request = match command {
        Commands::Status => ("system.status", json!({})),
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
            TaskCommands::List { project } => ("tasks.list", json!({ "project": project })),
            TaskCommands::Show { task_id } => ("tasks.get", json!({ "taskId": task_id })),
            TaskCommands::Cancel { task_id } => ("tasks.cancel", json!({ "taskId": task_id })),
            TaskCommands::Message { task_id, text } => (
                "tasks.message",
                json!({ "taskId": task_id, "text": text }),
            ),
            TaskCommands::Messages { task_id } => {
                ("tasks.messages", json!({ "taskId": task_id }))
            }
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
            AgentCommands::List { project } => ("agents.list", json!({ "project": project })),
            AgentCommands::Start {
                role,
                mode,
                project,
                fresh,
            } => (
                "agents.start",
                json!({ "role": role, "mode": mode.as_str(), "project": project, "fresh": fresh }),
            ),
            AgentCommands::Show { bot_id } => ("agents.show", json!({ "botId": bot_id })),
            AgentCommands::Attach { bot_id } => ("agents.attach", json!({ "botId": bot_id })),
            AgentCommands::Stop { bot_id } => ("agents.stop", json!({ "botId": bot_id })),
            AgentCommands::Prompt { bot_id, text } => {
                ("agents.prompt", json!({ "botId": bot_id, "text": text }))
            }
            AgentCommands::Interrupt { bot_id } => {
                ("agents.interrupt", json!({ "botId": bot_id }))
            }
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
        Commands::Gui | Commands::Tui => unreachable!("UI commands are handled before RPC"),
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
        .and_then(|value| render::render("internal.sessions.start", &value))
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

fn socket_path(override_path: Option<PathBuf>) -> PathBuf {
    if let Some(path) = override_path {
        return path;
    }
    if let Some(path) = env::var_os("KBOTS_SOCKET") {
        return PathBuf::from(path);
    }
    if let Some(runtime_dir) = env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime_dir).join("kbots.sock");
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/run/kbots.sock")
}

#[cfg(unix)]
fn request(socket: &std::path::Path, method: &str, params: Value) -> Result<Value, Box<dyn Error>> {
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(socket).map_err(|error| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "cannot connect to daemon socket {}: {error}; start the kbots daemon or set KBOTS_SOCKET",
                socket.display()
            ),
        )
    })?;
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
        return Err(format!("daemon returned an error: {error}").into());
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

fn run_ui(env_name: &str, binary_name: &str) -> Result<(), Box<dyn Error>> {
    let executable = env::var_os(env_name).map(PathBuf::from).unwrap_or_else(|| {
        env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join(binary_name)))
            .filter(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from(binary_name))
    });
    let status = Command::new(&executable).status().map_err(|error| {
        io::Error::other(format!(
            "cannot start {} at {}: {error}",
            binary_name,
            executable.display()
        ))
    })?;
    process::exit(status.code().unwrap_or(1));
}
