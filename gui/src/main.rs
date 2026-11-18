use serde_json::{Value, json};
use slint::ComponentHandle;
use std::{
    collections::{HashMap, HashSet},
    env,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

mod terminal;
use terminal::{HcClient, ScreenCell, TerminalEvent};

slint::include_modules!();

fn socket_path() -> PathBuf {
    let mut args = env::args().skip(1);
    let mut explicit = None;
    while let Some(arg) = args.next() {
        if let Some(path) = arg.strip_prefix("--socket=") {
            explicit = Some(PathBuf::from(path));
            break;
        }
        if arg == "--socket" {
            explicit = args.next().map(PathBuf::from);
            break;
        }
    }
    explicit
        .or_else(|| env::var_os("KBOTS_SOCKET").map(PathBuf::from))
        .or_else(|| env::var_os("XDG_RUNTIME_DIR").map(|p| PathBuf::from(p).join("kbots.sock")))
        .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/run/kbots.sock")))
        .unwrap_or_else(|| PathBuf::from("/tmp/kbots.sock"))
}

fn rpc(method: &str, params: Value) -> Result<Value, String> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path)
        .map_err(|e| format!("Cannot connect to {}: {e}", path.display()))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(4)))
        .map_err(|e| e.to_string())?;
    stream
        .write_all(
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params})
            )
            .as_bytes(),
        )
        .map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let response: Value =
        serde_json::from_str(&line).map_err(|e| format!("Invalid daemon response: {e}"))?;
    if let Some(error) = response.get("error") {
        return Err(error.to_string());
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| "Daemon response has no result".into())
}

fn sstr(value: &Value, key: &str) -> String {
    value
        .get(key)
        .map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        })
        .unwrap_or_default()
}

fn opt_str(value: &Value, key: &str) -> String {
    value
        .get(key)
        .map(|v| match v {
            Value::Null => "—".to_string(),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .unwrap_or_else(|| "—".to_string())
}

fn num(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn shorten(text: &str, keep: usize) -> String {
    let count = text.chars().count();
    if count <= keep {
        return text.to_string();
    }
    format!("{}…", text.chars().take(keep).collect::<String>())
}

fn fmt_uptime(seconds: i64) -> String {
    if seconds < 0 {
        return "—".to_string();
    }
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let rest = seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {rest}s")
    } else {
        format!("{rest}s")
    }
}

fn scalar_fmt(value: &Value) -> String {
    match value {
        Value::Null => "—".to_string(),
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(scalar_fmt).collect::<Vec<_>>().join(", "),
        Value::Object(_) => serde_json::to_string(value).unwrap_or_default(),
        other => other.to_string(),
    }
}

fn kv_dump(value: &Value) -> String {
    match value {
        Value::Object(map) => map
            .iter()
            .map(|(key, item)| format!("{key}: {}", scalar_fmt(item)))
            .collect::<Vec<_>>()
            .join("\n"),
        other => scalar_fmt(other),
    }
}

/// Reports an RPC outcome to the connection badge and notice line. Returns the
/// result value on success so callers can apply it to their own view.
fn rpc_ok(ui: &MainWindow, result: Result<Value, String>) -> Option<Value> {
    match result {
        Ok(value) => {
            ui.set_connection_text("Connected".into());
            ui.set_connection_ok(true);
            ui.set_notice("".into());
            ui.set_detail("".into());
            Some(value)
        }
        Err(error) => {
            ui.set_connection_text("Disconnected".into());
            ui.set_connection_ok(false);
            ui.set_notice(error.into());
            ui.set_detail("".into());
            None
        }
    }
}

fn apply_status(ui: &MainWindow, value: &Value) {
    let daemon = value.get("daemon").cloned().unwrap_or(Value::Null);
    let tasks = value.get("tasks").cloned().unwrap_or(Value::Null);
    let by_status = tasks.get("byStatus").cloned().unwrap_or(Value::Null);
    let planning = num(&by_status, "planning");
    let in_progress = num(&by_status, "in_progress");
    let completed = num(&by_status, "completed");
    let canceled = num(&by_status, "canceled");
    let total = num(&tasks, "total");
    let uptime_seconds = num(&daemon, "uptimeSeconds");
    let name = sstr(&daemon, "name");
    let version = sstr(&daemon, "version");
    ui.set_status(StatusInfo {
        name: name.clone().into(),
        version: version.clone().into(),
        uptime: fmt_uptime(uptime_seconds).into(),
        projects: num(value, "projects") as i32,
        tasks_planning: planning as i32,
        tasks_in_progress: in_progress as i32,
        tasks_completed: completed as i32,
        tasks_canceled: canceled as i32,
        pending_approvals: num(value, "pendingApprovals") as i32,
        open_detail: format!("{planning} planning · {in_progress} in progress · {total} total").into(),
        totals_line: format!("{completed} completed · {canceled} canceled").into(),
        daemon_line: format!(
            "{name} v{version} · pid {} · socket {}",
            num(&daemon, "pid"),
            sstr(&daemon, "socketPath")
        )
        .into(),
    });
    ui.set_has_status(true);
}

fn apply_projects(ui: &MainWindow, value: &Value) {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mapped = items
        .iter()
        .map(|item| ProjectItem {
            id: sstr(item, "projectId").into(),
            name: sstr(item, "name").into(),
            path: sstr(item, "path").into(),
            bot_roles: item
                .get("botRoles")
                .map(|v| match v {
                    Value::Array(roles) => roles
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", "),
                    other => other.to_string(),
                })
                .unwrap_or_default()
                .into(),
            created: sstr(item, "createdAt").into(),
        })
        .collect::<Vec<_>>();
    ui.set_projects(std::rc::Rc::new(slint::VecModel::from(mapped)).into());
}

fn apply_tasks(ui: &MainWindow, value: &Value) {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mapped = items
        .iter()
        .map(|item| TaskItem {
            id: sstr(item, "taskId").into(),
            title: sstr(item, "title").into(),
            status: sstr(item, "status").into(),
            project: sstr(item, "projectId").into(),
            created: sstr(item, "createdAt").into(),
        })
        .collect::<Vec<_>>();
    ui.set_tasks(std::rc::Rc::new(slint::VecModel::from(mapped)).into());
}

fn apply_agents(ui: &MainWindow, value: &Value) {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let bindings = items
        .iter()
        .map(|item| {
            let registered = item.get("registered").and_then(Value::as_bool).unwrap_or(false);
            let project = item
                .get("projectName")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| shorten(&sstr(item, "projectPath"), 48));
            let enabled = match item.get("enabled") {
                Some(Value::Bool(true)) => "enabled",
                Some(Value::Bool(false)) => "disabled",
                _ => "—",
            };
            let session = opt_str(item, "sessionID");
            let session = if session == "—" {
                session
            } else {
                shorten(&session, 12)
            };
            let herdr = item
                .get("herdr")
                .map(|herdr| {
                    [
                        sstr(herdr, "state"),
                        sstr(herdr, "status"),
                    ]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ")
                })
                .unwrap_or_default();
            AgentBindingItem {
                project: project.into(),
                role: sstr(item, "role").into(),
                enabled: enabled.into(),
                session: session.into(),
                herdr: herdr.into(),
                registered,
            }
        })
        .collect::<Vec<_>>();
    ui.set_agent_bindings(std::rc::Rc::new(slint::VecModel::from(bindings)).into());

    let managed = value
        .get("managed")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mapped = managed
        .iter()
        .map(|item| ManagedAgentItem {
            id: sstr(item, "id").into(),
            project: item
                .get("projectName")
                .or_else(|| item.get("projectId"))
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_else(|| "—".to_string())
                .into(),
            role: sstr(item, "role").into(),
            mode: sstr(item, "mode").into(),
            status: sstr(item, "status").into(),
            name: sstr(item, "name").into(),
            last_error: sstr(item, "lastError").into(),
        })
        .collect::<Vec<_>>();
    ui.set_managed_agents(std::rc::Rc::new(slint::VecModel::from(mapped)).into());

    if let Some(unavailable) = value.get("herdrUnavailable").and_then(Value::as_str) {
        ui.set_notice(format!("herdr unavailable: {unavailable}").into());
    }
}

fn apply_approvals(ui: &MainWindow, value: &Value) {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mapped = items
        .iter()
        .map(|item| {
            let target = [
                sstr(item, "projectId"),
                sstr(item, "taskId"),
            ]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("  ·  ");
            ApprovalItem {
                id: sstr(item, "approvalId").into(),
                operation: sstr(item, "operation").into(),
                detail: sstr(item, "detail").into(),
                target: target.into(),
                status: sstr(item, "status").into(),
                created: sstr(item, "createdAt").into(),
            }
        })
        .collect::<Vec<_>>();
    ui.set_approvals(std::rc::Rc::new(slint::VecModel::from(mapped)).into());
}

fn apply_events(ui: &MainWindow, value: &Value) {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mapped = items
        .iter()
        .map(|item| EventItem {
            ts: sstr(item, "ts").into(),
            kind: sstr(item, "kind").into(),
            message: sstr(item, "message").into(),
        })
        .collect::<Vec<_>>();
    ui.set_events(std::rc::Rc::new(slint::VecModel::from(mapped)).into());
}

/// Best-effort list reloads used after a mutation so the notice keeps showing
/// the mutation result; failures are silent and leave the model untouched.
fn refresh_projects(ui: &MainWindow) {
    if let Ok(value) = rpc("projects.list", json!({})) {
        apply_projects(ui, &value);
    }
}

fn refresh_tasks(ui: &MainWindow, filter: &str) {
    let params = if filter.is_empty() {
        json!({})
    } else {
        json!({"project": filter})
    };
    if let Ok(value) = rpc("tasks.list", params) {
        apply_tasks(ui, &value);
    }
}

fn refresh_approvals(ui: &MainWindow) {
    if let Ok(value) = rpc("approvals.list", json!({})) {
        apply_approvals(ui, &value);
    }
}

fn render_terminal_cells(ui: &MainWindow, cells: Vec<ScreenCell>) {
    let cells = cells
        .into_iter()
        .map(|cell| TerminalCell {
            text: cell.text.into(),
            foreground: cell.foreground,
            background: cell.background,
            bold: cell.bold,
            italic: cell.italic,
            underline: cell.underline,
        })
        .collect::<Vec<_>>();
    ui.set_terminal_cells(std::rc::Rc::new(slint::VecModel::from(cells)).into());
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;
    let hc_sessions = Arc::new(Mutex::new(HashMap::<String, (HcClient, u32, bool)>::new()));
    let active_terminal = Arc::new(Mutex::new(String::new()));
    let attached_addresses = Arc::new(Mutex::new(Vec::<String>::new()));
    let allowed_terminals = Arc::new(Mutex::new(HashSet::<String>::new()));
    let (terminal_tx, terminal_rx) = mpsc::sync_channel(2);
    let terminal_rx = Arc::new(Mutex::new(terminal_rx));
    let weak = ui.as_weak();
    ui.on_refresh(move || {
        if let Some(ui) = weak.upgrade() {
            if let Some(value) = rpc_ok(&ui, rpc("system.status", json!({}))) {
                apply_status(&ui, &value);
            }
        }
    });

    let weak = ui.as_weak();
    ui.on_load_tasks(move || {
        if let Some(ui) = weak.upgrade() {
            let project = ui.get_filter_project().trim().to_string();
            if let Some(value) = rpc_ok(&ui, rpc("tasks.list", if project.is_empty() { json!({}) } else { json!({"project": project}) })) {
                apply_tasks(&ui, &value);
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_create_task(move || {
        if let Some(ui) = weak.upgrade() {
            let title = ui.get_task_title().trim().to_string();
            let project = ui.get_task_project().trim().to_string();
            if title.is_empty() {
                ui.set_notice("Enter a task description first.".into());
                return;
            }
            let mut params = json!({"title":title});
            if !project.is_empty() {
                params["project"] = json!(project);
            }
            if let Some(value) = rpc_ok(&ui, rpc("tasks.create", params)) {
                let task_id = sstr(&value, "taskId");
                ui.set_notice(format!("Created {task_id}").into());
                refresh_tasks(&ui, &ui.get_filter_project().trim().to_string());
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_cancel_task(move || {
        if let Some(ui) = weak.upgrade() {
            let id = ui.get_item_id().trim().to_string();
            if id.is_empty() {
                ui.set_notice("Enter a task ID first.".into());
                return;
            }
            if let Some(value) = rpc_ok(&ui, rpc("tasks.cancel", json!({"taskId":id}))) {
                let task_id = sstr(&value, "taskId");
                ui.set_notice(format!("Canceled {task_id}").into());
                refresh_tasks(&ui, &ui.get_filter_project().trim().to_string());
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_load_projects(move || {
        if let Some(ui) = weak.upgrade() {
            if let Some(value) = rpc_ok(&ui, rpc("projects.list", json!({}))) {
                apply_projects(&ui, &value);
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_register_project(move || {
        if let Some(ui) = weak.upgrade() {
            let path = ui.get_project_path().trim().to_string();
            let name = ui.get_project_name().trim().to_string();
            if path.is_empty() {
                ui.set_notice("Enter an existing project directory.".into());
                return;
            }
            let mut params = json!({"path":path,"botRoles":["ceo","git","ops"]});
            if !name.is_empty() {
                params["name"] = json!(name);
            }
            if let Some(value) = rpc_ok(&ui, rpc("projects.register", params)) {
                let project_id = sstr(&value, "projectId");
                let created = value.get("created").and_then(Value::as_bool).unwrap_or(false);
                ui.set_notice(
                    if created {
                        format!("Registered new project {project_id}")
                    } else {
                        format!("Updated existing registration {project_id}")
                    }
                    .into(),
                );
                refresh_projects(&ui);
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_configure_project(move || {
        if let Some(ui) = weak.upgrade() {
            let path = ui.get_project_path().trim().to_string();
            if path.is_empty() {
                ui.set_notice("Enter the project directory to configure.".into());
                return;
            }
            let weak = ui.as_weak();
            thread::spawn(move || {
                let result = Command::new("kbots")
                    .args(["project", "configure", &path])
                    .output();
                let message = match result {
                    Ok(output) => format!(
                        "{}{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ),
                    Err(error) => format!("Could not run kbots project configure: {error}"),
                };
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.set_notice("".into());
                    ui.set_detail(message.into());
                });
            });
        }
    });
    let weak = ui.as_weak();
    ui.on_start_sessions(move || {
        if let Some(ui) = weak.upgrade() {
            let path = ui.get_project_path().trim().to_string();
            if path.is_empty() {
                ui.set_notice("Enter the project directory to start its bot sessions.".into());
                return;
            }
            let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../src/backend/cli/start-sessions.ts");
            let weak = ui.as_weak();
            thread::spawn(move || {
                let result = Command::new("bun")
                    .arg("run")
                    .arg(script)
                    .args(["--directory", &path])
                    .output();
                let message = match result {
                    Ok(output) => format!(
                        "{}{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ),
                    Err(error) => format!("Could not start bot sessions: {error}"),
                };
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.set_notice("".into());
                    ui.set_detail(message.into());
                });
            });
        }
    });
    let weak = ui.as_weak();
    ui.on_load_agents(move || {
        if let Some(ui) = weak.upgrade() {
            if let Some(value) = rpc_ok(&ui, rpc("agents.list", json!({}))) {
                apply_agents(&ui, &value);
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_load_approvals(move || {
        if let Some(ui) = weak.upgrade() {
            if let Some(value) = rpc_ok(&ui, rpc("approvals.list", json!({}))) {
                apply_approvals(&ui, &value);
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_approve(move || {
        if let Some(ui) = weak.upgrade() {
            let id = ui.get_item_id().trim().to_string();
            if id.is_empty() {
                ui.set_notice("Enter an approval ID first.".into());
                return;
            }
            if let Some(value) = rpc_ok(&ui, rpc("approvals.approve", json!({"approvalId":id}))) {
                let approval_id = sstr(&value, "approvalId");
                ui.set_notice(format!("Approved {approval_id}").into());
                refresh_approvals(&ui);
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_load_activity(move || {
        if let Some(ui) = weak.upgrade() {
            if let Some(value) = rpc_ok(&ui, rpc("events.list", json!({"limit":100}))) {
                apply_events(&ui, &value);
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_show_item(move || {
        if let Some(ui) = weak.upgrade() {
            let id = ui.get_item_id().trim().to_string();
            if id.is_empty() {
                ui.set_notice("Enter a task or project ID first.".into());
                return;
            }
            let page = ui.get_active_page().to_string();
            let (method, key) = if page == "Projects" {
                ("projects.get", "projectId")
            } else {
                ("tasks.get", "taskId")
            };
            if let Some(value) = rpc_ok(&ui, rpc(method, json!({key:id}))) {
                ui.set_detail(kv_dump(&value).into());
            }
        }
    });

    let weak = ui.as_weak();
    let allowed = allowed_terminals.clone();
    ui.on_refresh_terminals(move || {
        let weak = weak.clone();
        let allowed = allowed.clone();
        thread::spawn(move || {
            let result = rpc("agents.list", json!({})).and_then(|agents| {
                let addresses = agents
                    .get("liveTerminals")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|terminal| {
                        terminal
                            .get("address")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .collect::<HashSet<_>>();
                let mut hc = HcClient::connect()?;
                let terminals = hc
                    .list()?
                    .into_iter()
                    .filter(|terminal| addresses.contains(&terminal.address))
                    .collect::<Vec<_>>();
                *allowed
                    .lock()
                    .map_err(|_| "terminal allowlist lock was poisoned")? = addresses;
                Ok(terminals)
            });
            let _ = weak.upgrade_in_event_loop(move |ui| match result {
                Ok(terminals) => {
                    let items = terminals
                        .into_iter()
                        .map(|terminal| TerminalItem {
                            address: terminal.address.into(),
                            name: terminal.name.into(),
                            cwd: terminal.cwd.into(),
                            agent: terminal.agent.into(),
                            status: terminal.status.into(),
                        })
                        .collect::<Vec<_>>();
                    ui.set_terminals(std::rc::Rc::new(slint::VecModel::from(items)).into());
                    ui.set_terminal_status("Herdr Core connected".into());
                }
                Err(error) => ui.set_terminal_status(error.into()),
            });
        });
    });

    let weak = ui.as_weak();
    let sessions = hc_sessions.clone();
    let active = active_terminal.clone();
    let attached = attached_addresses.clone();
    let allowed = allowed_terminals.clone();
    let events = terminal_tx.clone();
    ui.on_attach_terminal(move || {
        let Some(ui) = weak.upgrade() else { return; };
        let address = ui.get_terminal_address().trim().to_string();
        let cols = ui.get_terminal_cols().clamp(1, u16::MAX as i32) as u16;
        let rows = ui.get_terminal_rows().clamp(1, u16::MAX as i32) as u16;
        let observe = ui.get_terminal_observe();
        if address.is_empty() { ui.set_terminal_status("Choose a live terminal first.".into()); return; }
        let weak = weak.clone();
        let sessions = sessions.clone();
        let active = active.clone();
        let attached = attached.clone();
        let allowed = allowed.clone();
        let events = events.clone();
        thread::spawn(move || {
            if !allowed.lock().map(|items| items.contains(&address)).unwrap_or(false) {
                let _ = weak.upgrade_in_event_loop(move |ui| ui.set_terminal_status("Discover terminals first; only terminals inside registered projects can be attached.".into()));
                return;
            }
            let result = (|| -> Result<u32, String> {
                let previous_address = active.lock().map_err(|_| "active terminal lock was poisoned")?.clone();
                let mut active_sessions = sessions.lock().map_err(|_| "terminal sessions lock was poisoned")?;
                if previous_address != address {
                    if let Some(previous) = active_sessions.get(&previous_address) { let _ = previous.0.focus(previous.1, false); }
                }
                if let Some((hc, channel, current_observe)) = active_sessions.get(&address) {
                    if *current_observe != observe {
                        hc.detach(*channel);
                        active_sessions.remove(&address);
                    } else {
                    let _ = hc.focus(*channel, true);
                    let _ = hc.resize(*channel, cols, rows);
                    *active.lock().map_err(|_| "active terminal lock was poisoned")? = address.clone();
                    return Ok(*channel);
                    }
                }
                let mut hc = HcClient::connect()?;
                let attached_channel = hc.attach(&address, cols, rows, observe, events)?;
                let _ = hc.focus(attached_channel, true);
                active_sessions.insert(address.clone(), (hc, attached_channel, observe));
                *active.lock().map_err(|_| "active terminal lock was poisoned")? = address.clone();
                if let Ok(mut addresses) = attached.lock() {
                    if !addresses.contains(&address) { addresses.push(address.clone()); }
                }
                Ok(attached_channel)
            })();
            match result {
                Ok(_) => {
                    let tabs = attached.lock().map(|addresses| addresses.iter().cloned().map(Into::into).collect::<Vec<slint::SharedString>>()).unwrap_or_default();
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        ui.set_attached_terminals(std::rc::Rc::new(slint::VecModel::from(tabs)).into());
                        ui.set_terminal_status(format!("Attached to {address}").into());
                    });
                }
                Err(error) => { let _ = weak.upgrade_in_event_loop(move |ui| ui.set_terminal_status(error.into())); }
            }
        });
    });

    let sessions = hc_sessions.clone();
    let active = active_terminal.clone();
    ui.on_terminal_input(move |input| {
        let current = active.lock().map(|value| value.clone()).unwrap_or_default();
        if let Ok(sessions) = sessions.lock() {
            if let Some((client, channel, _)) = sessions.get(&current) {
                let _ = client.send_input(*channel, input.as_bytes());
            }
        }
    });

    let sessions = hc_sessions.clone();
    let active = active_terminal.clone();
    ui.on_terminal_resize(move |cols, rows| {
        let current = active.lock().map(|value| value.clone()).unwrap_or_default();
        if let Ok(sessions) = sessions.lock() {
            if let Some((client, channel, _)) = sessions.get(&current) {
                let _ = client.resize(*channel, cols as u16, rows as u16);
            }
        }
    });

    let sessions = hc_sessions.clone();
    let active = active_terminal.clone();
    ui.on_terminal_focus(move |focused| {
        let current = active.lock().map(|value| value.clone()).unwrap_or_default();
        if let Ok(sessions) = sessions.lock() {
            if let Some((client, channel, _)) = sessions.get(&current) {
                let _ = client.focus(*channel, focused);
            }
        }
    });

    let sessions = hc_sessions.clone();
    let active = active_terminal.clone();
    ui.on_terminal_scrollback(move |delta| {
        let current = active.lock().map(|value| value.clone()).unwrap_or_default();
        if let Ok(sessions) = sessions.lock() {
            if let Some((client, _, _)) = sessions.get(&current) {
                client.scrollback(delta);
            }
        }
    });

    let weak = ui.as_weak();
    let rx = terminal_rx.clone();
    let active = active_terminal.clone();
    let sessions = hc_sessions.clone();
    let attached = attached_addresses.clone();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(50),
        move || {
            let Ok(receiver) = rx.lock() else {
                return;
            };
            while let Ok(event) = receiver.try_recv() {
                let weak = weak.clone();
                match event {
                    TerminalEvent::Attached { address, channel } => {
                        let is_active = active
                            .lock()
                            .map(|current| *current == address)
                            .unwrap_or(false)
                            && sessions
                                .lock()
                                .map(|items| {
                                    items.get(&address).is_some_and(|(_, active_channel, _)| {
                                        *active_channel == channel
                                    })
                                })
                                .unwrap_or(false);
                        if is_active {
                            let _ = weak.upgrade_in_event_loop(move |ui| {
                                ui.set_terminal_status(format!("Attached to {address}").into())
                            });
                        }
                    }
                    TerminalEvent::Screen {
                        address,
                        channel,
                        cells,
                    } => {
                        let is_current_session = sessions
                            .lock()
                            .map(|items| {
                                items.get(&address).is_some_and(|(_, active_channel, _)| {
                                    *active_channel == channel
                                })
                            })
                            .unwrap_or(false);
                        let is_active = is_current_session
                            && active
                                .lock()
                                .map(|current| *current == address)
                                .unwrap_or(false);
                        if is_active {
                            let _ = weak
                                .upgrade_in_event_loop(move |ui| render_terminal_cells(&ui, cells));
                        }
                    }
                    TerminalEvent::Closed {
                        address,
                        channel,
                        reason,
                    } => {
                        let was_current_session = if let Ok(mut sessions) = sessions.lock() {
                            if sessions
                                .get(&address)
                                .is_some_and(|(_, active_channel, _)| *active_channel == channel)
                            {
                                sessions.remove(&address);
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        };
                        if was_current_session {
                            if let Ok(mut attached) = attached.lock() {
                                attached.retain(|item| item != &address);
                            }
                        }
                        let is_active = was_current_session
                            && active
                                .lock()
                                .map(|current| *current == address)
                                .unwrap_or(false);
                        let tabs = attached
                            .lock()
                            .map(|addresses| {
                                addresses
                                    .iter()
                                    .cloned()
                                    .map(Into::into)
                                    .collect::<Vec<slint::SharedString>>()
                            })
                            .unwrap_or_default();
                        let _ = weak.upgrade_in_event_loop(move |ui| {
                            ui.set_attached_terminals(
                                std::rc::Rc::new(slint::VecModel::from(tabs)).into(),
                            );
                            if is_active {
                                ui.set_terminal_status(
                                    format!("Terminal stream closed: {reason}").into(),
                                );
                            }
                        });
                    }
                    TerminalEvent::Error {
                        address,
                        channel,
                        message,
                    } => {
                        let was_current_session = if let Ok(mut sessions) = sessions.lock() {
                            if sessions
                                .get(&address)
                                .is_some_and(|(_, active_channel, _)| *active_channel == channel)
                            {
                                sessions.remove(&address);
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        };
                        if was_current_session {
                            if let Ok(mut attached) = attached.lock() {
                                attached.retain(|item| item != &address);
                            }
                        }
                        let is_active = was_current_session
                            && active
                                .lock()
                                .map(|current| *current == address)
                                .unwrap_or(false);
                        let tabs = attached
                            .lock()
                            .map(|addresses| {
                                addresses
                                    .iter()
                                    .cloned()
                                    .map(Into::into)
                                    .collect::<Vec<slint::SharedString>>()
                            })
                            .unwrap_or_default();
                        let _ = weak.upgrade_in_event_loop(move |ui| {
                            ui.set_attached_terminals(
                                std::rc::Rc::new(slint::VecModel::from(tabs)).into(),
                            );
                            if is_active {
                                ui.set_terminal_status(message.into());
                            }
                        });
                    }
                }
            }
        },
    );
    let run_result = ui.run();
    if let Ok(sessions) = hc_sessions.lock() {
        for (client, channel, _) in sessions.values() {
            client.detach(*channel);
        }
    }
    run_result
}
