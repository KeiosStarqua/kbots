use std::{
    env,
    error::Error,
    io::{self, BufRead, BufReader, Write},
    path::PathBuf,
    process::{self, Command},
    time::Duration,
};

use clap::{Parser, Subcommand};
use serde_json::{Value, json};

#[derive(Debug, Parser)]
#[command(version, about = "Automation CLI for the kbots coding workflow")]
struct Cli {
    /// Override the daemon's Unix socket path (or set KBOTS_SOCKET).
    #[arg(long, global = true, value_name = "PATH")]
    socket: Option<PathBuf>,

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
    Agents {
        #[command(subcommand)]
        command: AgentCommands,
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
        #[arg(long)]
        project: Option<String>,
    },
    /// List tasks, optionally filtered by project.
    List {
        #[arg(long)]
        project: Option<String>,
    },
    /// Show one task.
    Show { task_id: String },
    /// Cancel one task.
    Cancel { task_id: String },
}

#[derive(Debug, Subcommand)]
enum AgentCommands {
    /// List agent roles, active runs, and session identifiers.
    List,
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("kbots-cli: {error}");
        process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    match cli.command {
        Commands::Gui => run_ui("KBOTS_GUI_BIN", "kbots-gui"),
        Commands::Tui => run_ui("KBOTS_TUI_BIN", "kbots-tui"),
        command => {
            let (method, params) = rpc_call(command);
            let socket = socket_path(cli.socket);
            let response = request(&socket, method, params)?;
            println!("{}", serde_json::to_string_pretty(&response)?);
            Ok(())
        }
    }
}

fn rpc_call(command: Commands) -> (&'static str, Value) {
    match command {
        Commands::Status => ("system.status", json!({})),
        Commands::Tasks { command } => match command {
            TaskCommands::Add { title, project } => (
                "tasks.create",
                json!({ "title": title, "project": project }),
            ),
            TaskCommands::List { project } => ("tasks.list", json!({ "project": project })),
            TaskCommands::Show { task_id } => ("tasks.get", json!({ "taskId": task_id })),
            TaskCommands::Cancel { task_id } => ("tasks.cancel", json!({ "taskId": task_id })),
        },
        Commands::Agents {
            command: AgentCommands::List,
        } => ("agents.list", json!({})),
        Commands::Approve { approval_id } => {
            ("approvals.approve", json!({ "approvalId": approval_id }))
        }
        Commands::Gui | Commands::Tui => unreachable!("UI commands are handled before RPC"),
    }
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
