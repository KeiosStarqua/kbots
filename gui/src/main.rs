use serde_json::{Value, json};
use slint::ComponentHandle;
use std::{env, io::{BufRead, BufReader, Write}, os::unix::net::UnixStream, path::PathBuf, time::Duration};

slint::include_modules!();

fn socket_path() -> PathBuf {
    env::args().skip(1).find_map(|arg| arg.strip_prefix("--socket=").map(PathBuf::from))
        .or_else(|| env::var_os("KBOTS_SOCKET").map(PathBuf::from))
        .or_else(|| env::var_os("XDG_RUNTIME_DIR").map(|p| PathBuf::from(p).join("kbots.sock")))
        .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/run/kbots.sock")))
        .unwrap_or_else(|| PathBuf::from("/tmp/kbots.sock"))
}

fn rpc(method: &str, params: Value) -> Result<Value, String> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path).map_err(|e| format!("Cannot connect to {}: {e}", path.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(4))).map_err(|e| e.to_string())?;
    stream.write_all(format!("{}\n", json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params})).as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).map_err(|e| e.to_string())?;
    let response: Value = serde_json::from_str(&line).map_err(|e| format!("Invalid daemon response: {e}"))?;
    if let Some(error) = response.get("error") { return Err(error.to_string()); }
    response.get("result").cloned().ok_or_else(|| "Daemon response has no result".into())
}

fn display_result(ui: &MainWindow, result: Result<Value, String>) {
    match result {
        Ok(value) => {
            ui.set_connection_text("Connected".into());
            ui.set_connection_ok(true);
            ui.set_output(serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()).into());
        }
        Err(error) => {
            ui.set_connection_text("Disconnected".into());
            ui.set_connection_ok(false);
            ui.set_output(error.into());
        }
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;
    let weak = ui.as_weak();
    ui.on_refresh(move || if let Some(ui) = weak.upgrade() { display_result(&ui, rpc("system.status", json!({}))); });

    let weak = ui.as_weak();
    ui.on_load_tasks(move || if let Some(ui) = weak.upgrade() {
        let project = ui.get_filter_project().trim().to_string();
        display_result(&ui, rpc("tasks.list", if project.is_empty() { json!({}) } else { json!({"project":project}) }));
    });
    let weak = ui.as_weak();
    ui.on_create_task(move || if let Some(ui) = weak.upgrade() {
        let title = ui.get_task_title().trim().to_string(); let project = ui.get_task_project().trim().to_string();
        if title.is_empty() { ui.set_output("Enter a task description first.".into()); return; }
        let mut params = json!({"title":title}); if !project.is_empty() { params["project"] = json!(project); }
        display_result(&ui, rpc("tasks.create", params));
    });
    let weak = ui.as_weak();
    ui.on_cancel_task(move || if let Some(ui) = weak.upgrade() {
        let id = ui.get_item_id().trim().to_string(); if id.is_empty() { ui.set_output("Enter a task ID first.".into()); return; }
        display_result(&ui, rpc("tasks.cancel", json!({"taskId":id})));
    });
    let weak = ui.as_weak();
    ui.on_load_projects(move || if let Some(ui) = weak.upgrade() { display_result(&ui, rpc("projects.list", json!({}))); });
    let weak = ui.as_weak();
    ui.on_register_project(move || if let Some(ui) = weak.upgrade() {
        let path = ui.get_project_path().trim().to_string(); let name = ui.get_project_name().trim().to_string();
        if path.is_empty() { ui.set_output("Enter an existing project directory.".into()); return; }
        display_result(&ui, rpc("projects.register", json!({"path":path,"name":name,"botRoles":["ceo","git","ops"]})));
    });
    let weak = ui.as_weak();
    ui.on_load_agents(move || if let Some(ui) = weak.upgrade() { display_result(&ui, rpc("agents.list", json!({}))); });
    let weak = ui.as_weak();
    ui.on_load_approvals(move || if let Some(ui) = weak.upgrade() { display_result(&ui, rpc("approvals.list", json!({}))); });
    let weak = ui.as_weak();
    ui.on_approve(move || if let Some(ui) = weak.upgrade() {
        let id = ui.get_item_id().trim().to_string(); if id.is_empty() { ui.set_output("Enter an approval ID first.".into()); return; }
        display_result(&ui, rpc("approvals.approve", json!({"approvalId":id})));
    });
    let weak = ui.as_weak();
    ui.on_load_activity(move || if let Some(ui) = weak.upgrade() { display_result(&ui, rpc("events.list", json!({"limit":100}))); });
    let weak = ui.as_weak();
    ui.on_show_item(move || if let Some(ui) = weak.upgrade() {
        let id = ui.get_item_id().trim().to_string(); if id.is_empty() { ui.set_output("Enter a task or project ID first.".into()); return; }
        let page = ui.get_active_page().to_string();
        let (method, key) = if page == "Projects" { ("projects.get", "projectId") } else { ("tasks.get", "taskId") };
        display_result(&ui, rpc(method, json!({key:id})));
    });
    ui.run()
}
