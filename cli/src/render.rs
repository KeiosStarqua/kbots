//! Human-readable rendering for a terminal. The CLI prints the daemon's JSON
//! verbatim when stdout is a pipe; on a TTY it renders tables and summaries
//! instead (same idea as `hc ls`).
use serde_json::Value;

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

pub fn render(method: &str, value: &Value) -> Option<String> {
    match method {
        "system.status" => Some(render_status(value)),
        "tasks.create" | "tasks.cancel" => Some(render_task_write(value)),
        "tasks.list" => Some(render_tasks_list(value)),
        "tasks.get" => Some(render_task_get(value)),
        "projects.register" => Some(render_project_register(value)),
        "projects.list" => Some(render_projects_list(value)),
        "projects.get" => Some(render_project_get(value)),
        "approvals.approve" => Some(render_approval(value)),
        "agents.list" => Some(render_agents(value)),
        "internal.sessions.start" => Some(render_sessions_start(value)),
        "internal.project.configure" => Some(render_project_configure(value)),
        _ => None,
    }
}

fn char_width(value: &str) -> usize {
    value.chars().count()
}

fn pad(cell: &str, width: usize) -> String {
    let mut out = String::from(cell);
    for _ in char_width(cell)..width {
        out.push(' ');
    }
    out
}

fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|header| char_width(header)).collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if let Some(entry) = widths.get_mut(index) {
                *entry = (*entry).max(char_width(cell));
            }
        }
    }
    let format_row = |cells: &[String]| {
        let mut out = String::new();
        for (index, cell) in cells.iter().enumerate() {
            if index > 0 {
                out.push_str("  ");
            }
            if index + 1 < cells.len() {
                out.push_str(&pad(cell, widths[index]));
            } else {
                out.push_str(cell);
            }
        }
        out
    };
    let mut lines = Vec::with_capacity(rows.len() + 1);
    let header_cells: Vec<String> = headers.iter().map(|header| header.to_uppercase()).collect();
    lines.push(format!("{BOLD}{}{RESET}", format_row(&header_cells)));
    for row in rows {
        lines.push(format_row(row));
    }
    lines.join("\n")
}

fn detail(pairs: &[(&str, String)]) -> String {
    let width = pairs.iter().map(|(key, _)| key.len()).max().unwrap_or(0);
    pairs
        .iter()
        .map(|(key, value)| format!("{DIM}{key:<width$}{RESET}  {value}", width = width))
        .collect::<Vec<_>>()
        .join("\n")
}

fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn nullable(value: &Value, key: &str) -> String {
    match value.get(key) {
        None | Some(Value::Null) => "-".to_owned(),
        Some(Value::String(text)) if text.is_empty() => "-".to_owned(),
        Some(other) => other.as_str().unwrap_or_default().to_owned(),
    }
}

fn as_bool(value: &Value) -> String {
    match value {
        Value::Bool(true) => "yes".to_owned(),
        Value::Bool(false) => "no".to_owned(),
        _ => "?".to_owned(),
    }
}

fn list_items(value: &Value) -> Vec<Value> {
    value
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').find(|part| !part.is_empty()).unwrap_or(path)
}

fn render_status(value: &Value) -> String {
    let daemon = value.get("daemon").cloned().unwrap_or(Value::Null);
    let name = daemon.get("name").and_then(Value::as_str).unwrap_or("daemon");
    let version = daemon.get("version").and_then(Value::as_str).unwrap_or("?");
    let pid = daemon.get("pid").and_then(Value::as_i64).unwrap_or(-1);
    let uptime = daemon.get("uptimeSeconds").and_then(Value::as_i64).unwrap_or(0);
    let socket = daemon.get("socketPath").and_then(Value::as_str).unwrap_or("?");

    let projects = value.get("projects").and_then(Value::as_i64).unwrap_or(0);
    let tasks_total = value
        .get("tasks")
        .and_then(|tasks| tasks.get("total"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let by_status = value
        .get("tasks")
        .and_then(|tasks| tasks.get("byStatus"))
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, count)| format!("{key} {}", count.as_i64().unwrap_or(0)))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let pending = value.get("pendingApprovals").and_then(Value::as_i64).unwrap_or(0);

    let uptime_text = if uptime >= 3600 {
        format!("{}h {}m", uptime / 3600, (uptime % 3600) / 60)
    } else if uptime >= 60 {
        format!("{}m {}s", uptime / 60, uptime % 60)
    } else {
        format!("{uptime}s")
    };

    let mut lines = vec![
        format!("{BOLD}{name}{RESET} v{version} (pid {pid}, up {uptime_text})"),
        format!("socket    {socket}"),
        format!("projects  {projects}"),
        format!("tasks     {tasks_total}{}", if by_status.is_empty() { String::new() } else { format!(" ({by_status})") }),
        format!("pending approvals  {pending}"),
    ];
    if pending > 0 {
        lines.push(format!("{YELLOW}{pending} protected operation(s) await approval{RESET}"));
    }
    lines.join("\n")
}

fn render_task_write(value: &Value) -> String {
    let task_id = string(value, "taskId").unwrap_or_else(|| "?".to_owned());
    let status = string(value, "status").unwrap_or_else(|| "?".to_owned());
    if status == "canceled" {
        format!("{task_id} canceled")
    } else {
        format!("{task_id} created (status: {status})")
    }
}

fn render_tasks_list(value: &Value) -> String {
    let items = list_items(value);
    if items.is_empty() {
        return "No tasks.".to_owned();
    }
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|task| {
            vec![
                string(task, "taskId").unwrap_or_else(|| "?".to_owned()),
                string(task, "status").unwrap_or_else(|| "?".to_owned()),
                task.get("projectPath")
                    .and_then(Value::as_str)
                    .map(base_name)
                    .unwrap_or("-")
                    .to_owned(),
                string(task, "title").unwrap_or_default(),
            ]
        })
        .collect();
    table(&["ID", "STATUS", "PROJECT", "TITLE"], &rows)
}

fn render_task_get(value: &Value) -> String {
    detail(&[
        ("task", string(value, "taskId").unwrap_or_else(|| "?".to_owned())),
        ("title", string(value, "title").unwrap_or_default()),
        ("status", string(value, "status").unwrap_or_else(|| "?".to_owned())),
        (
            "project",
            match (string(value, "projectPath"), string(value, "projectId")) {
                (Some(path), Some(id)) => format!("{path} ({id})"),
                (Some(path), None) => path,
                _ => "-".to_owned(),
            },
        ),
        ("created", string(value, "createdAt").unwrap_or_else(|| "-".to_owned())),
        ("updated", string(value, "updatedAt").unwrap_or_else(|| "-".to_owned())),
        ("canceled", nullable(value, "canceledAt")),
    ])
}

fn render_project_register(value: &Value) -> String {
    let id = string(value, "projectId").unwrap_or_else(|| "?".to_owned());
    let name = string(value, "name").unwrap_or_else(|| "?".to_owned());
    let path = string(value, "path").unwrap_or_else(|| "?".to_owned());
    let action = if value.get("created").and_then(Value::as_bool).unwrap_or(false) {
        "registered"
    } else {
        "updated"
    };
    format!("{id} {action}: {name} ({path})")
}

fn render_projects_list(value: &Value) -> String {
    let items = list_items(value);
    if items.is_empty() {
        return "No projects registered.".to_owned();
    }
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|project| {
            vec![
                string(project, "projectId").unwrap_or_else(|| "?".to_owned()),
                string(project, "name").unwrap_or_default(),
                string(project, "path").unwrap_or_default(),
            ]
        })
        .collect();
    table(&["ID", "NAME", "PATH"], &rows)
}

fn render_project_get(value: &Value) -> String {
    let roles = value
        .get("botRoles")
        .and_then(Value::as_array)
        .map(|roles| {
            roles
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| "-".to_owned());
    detail(&[
        ("project", string(value, "projectId").unwrap_or_else(|| "?".to_owned())),
        ("name", string(value, "name").unwrap_or_else(|| "?".to_owned())),
        ("path", string(value, "path").unwrap_or_else(|| "?".to_owned())),
        ("roles", roles),
        ("created", string(value, "createdAt").unwrap_or_else(|| "-".to_owned())),
        ("updated", string(value, "updatedAt").unwrap_or_else(|| "-".to_owned())),
    ])
}

fn render_approval(value: &Value) -> String {
    let id = string(value, "approvalId").unwrap_or_else(|| "?".to_owned());
    let status = string(value, "status").unwrap_or_else(|| "?".to_owned());
    let already = value
        .get("alreadyApproved")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if already {
        format!("{id} already approved")
    } else {
        format!("{id} {status}")
    }
}

fn render_agents(value: &Value) -> String {
    let mut lines: Vec<String> = Vec::new();
    let items = list_items(value);
    if items.is_empty() {
        lines.push("No registered agents. See `kbots project add`.".to_owned());
    } else {
        lines.push(format!("{BOLD}Registered agents{RESET}"));
        let rows: Vec<Vec<String>> = items
            .iter()
            .map(|item| {
                let herdr = item.get("herdr").filter(|herdr| !herdr.is_null());
                let herdr_cell = match herdr {
                    Some(herdr) => {
                        let address = herdr.get("address").and_then(Value::as_str).unwrap_or("?");
                        let state = herdr
                            .get("status")
                            .or_else(|| herdr.get("state"))
                            .and_then(Value::as_str)
                            .unwrap_or("?");
                        let turn = herdr.get("turn").and_then(Value::as_i64);
                        match turn {
                            Some(turn) => format!("{address} {state} ({turn}t)"),
                            None => format!("{address} {state}"),
                        }
                    }
                    None => "-".to_owned(),
                };
                vec![
                    item.get("projectName")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            item.get("projectPath").and_then(Value::as_str).map(base_name).unwrap_or("-").to_owned()
                        }),
                    string(item, "role").unwrap_or_else(|| "?".to_owned()),
                    item.get("enabled").map(as_bool).unwrap_or_else(|| "?".to_owned()),
                    nullable(item, "sessionID"),
                    herdr_cell,
                ]
            })
            .collect();
        lines.push(table(&["PROJECT", "ROLE", "ENABLED", "SESSION", "HERDR"], &rows));
    }

    let live = value
        .get("liveTerminals")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    match value.get("herdrUnavailable") {
        Some(Value::String(reason)) => {
            lines.push(String::new());
            lines.push(format!("{YELLOW}live terminals unavailable: {reason}{RESET}"));
        }
        _ if !live.is_empty() => {
            lines.push(String::new());
            lines.push(format!("{BOLD}Live terminals (hc){RESET}"));
            let rows: Vec<Vec<String>> = live
                .iter()
                .map(|terminal| {
                    let cwd = terminal.get("cwd").and_then(Value::as_str).unwrap_or("?");
                    vec![
                        string(terminal, "address").unwrap_or_else(|| "?".to_owned()),
                        string(terminal, "agent").unwrap_or_else(|| "-".to_owned()),
                        string(terminal, "name").unwrap_or_else(|| "-".to_owned()),
                        base_name(cwd).to_owned(),
                        string(terminal, "status")
                            .or_else(|| string(terminal, "state"))
                            .unwrap_or_else(|| "?".to_owned()),
                        terminal
                            .get("turn")
                            .and_then(Value::as_i64)
                            .map(|turn| turn.to_string())
                            .unwrap_or_else(|| "-".to_owned()),
                    ]
                })
                .collect();
            lines.push(table(&["ADDRESS", "AGENT", "NAME", "WHERE", "STATUS", "TURN"], &rows));
        }
        _ => {}
    }
    lines.join("\n")
}

fn render_sessions_start(value: &Value) -> String {
    let project = string(value, "projectPath").unwrap_or_else(|| "?".to_owned());
    let mut lines = vec![format!("{BOLD}{project}{RESET}")];
    let mut any = false;
    for (key, label) in [("started", "started"), ("reused", "reused")] {
        if let Some(entries) = value.get(key).and_then(Value::as_array) {
            for entry in entries {
                any = true;
                let role = entry.get("role").and_then(Value::as_str).unwrap_or("?");
                let id = entry.get("sessionID").and_then(Value::as_str).unwrap_or("?");
                lines.push(format!("{label}  {role:<4}  {id}"));
            }
        }
    }
    let disabled = value
        .get("disabled")
        .and_then(Value::as_array)
        .map(|roles| {
            roles
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    if !disabled.is_empty() {
        lines.push(format!("disabled  {disabled}"));
    }
    if !any {
        lines.push("No sessions started.".to_owned());
    }
    lines.join("\n")
}

fn render_project_configure(value: &Value) -> String {
    let manifest = string(value, "manifest").unwrap_or_else(|| "?".to_owned());
    let format_list = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_array)
            .map(|entries| {
                let items: Vec<&str> = entries.iter().filter_map(Value::as_str).collect();
                if items.is_empty() {
                    "-".to_owned()
                } else {
                    items.join(", ")
                }
            })
            .unwrap_or_else(|| "-".to_owned())
    };
    detail(&[
        ("manifest", manifest),
        ("written", format_list("written")),
        ("preserved", format_list("preserved")),
        ("disabled", format_list("disabled")),
    ])
}
