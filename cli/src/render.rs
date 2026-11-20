//! Human-readable rendering for a terminal. The CLI prints the daemon's JSON
//! verbatim when stdout is a pipe; on a TTY it renders tables and summaries
//! instead (same idea as `hc ls`).
use serde_json::Value;

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

pub fn render(method: &str, value: &Value, show_finished: bool) -> Option<String> {
    match method {
        "system.status" => Some(format!(
            "{}{}",
            render_status(value),
            crate::render_bots::status_section(value)
        )),
        "tasks.create" | "tasks.cancel" => Some(render_task_write(value)),
        "tasks.list" => Some(render_tasks_list(value)),
        "tasks.get" => Some(render_task_get(value)),
        "tasks.message" => Some(render_task_message(value)),
        "tasks.messages" => Some(render_task_messages(value)),
        "tasks.brief" => Some(render_task_brief(value)),
        "tasks.inbox" => Some(render_tasks_inbox(value)),
        "tasks.update" => Some(render_task_update(value)),
        "projects.register" => Some(render_project_register(value)),
        "projects.list" => Some(render_projects_list(value)),
        "projects.get" => Some(render_project_get(value)),
        "approvals.approve" => Some(render_approval(value)),
        "approvals.list" => Some(render_approvals_list(value)),
        "events.list" => Some(render_events(value)),
        "agents.list" => Some(format!(
            "{}{}",
            render_agents(value, show_finished),
            crate::render_bots::agents_section(value)
        )),
        "agents.start" | "agents.show" => Some(format!(
            "{}{}",
            render_agent_detail(value),
            crate::render_bots::agent_section(value)
        )),
        "agents.logs" => Some(render_agent_logs(value)),
        "agents.stop" => Some(render_agent_action(value, "stopped")),
        "agents.prompt" => Some(render_agent_action(value, "accepted")),
        "agents.interrupt" => Some(render_agent_interrupt(value)),
        "watchers.list" => Some(render_watchers_list(value)),
        "watchers.set" => Some(render_watcher_set(value)),
        "internal.sessions.start" => Some(render_sessions_start(value)),
        "internal.project.configure" => Some(render_project_configure(value)),
        "internal.project.sync" => Some(render_project_sync(value)),
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

fn truncate_chars(text: &str, max: usize) -> String {
    if char_width(text) <= max {
        return text.to_owned();
    }
    let keep: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{keep}…")
}

fn lines_or_array(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(text)) => text
            .lines()
            .map(|line| line.trim_end().to_owned())
            .filter(|line| !line.is_empty())
            .collect(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(|line| line.trim_end().to_owned())
            .filter(|line| !line.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn text_or_array(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) if !text.trim().is_empty() => Some(text.clone()),
        Some(Value::Array(_)) => {
            let joined = lines_or_array(value).join("\n");
            if joined.is_empty() {
                None
            } else {
                Some(joined)
            }
        }
        _ => None,
    }
}

fn brief_item_detail(item: &Value) -> String {
    string(item, "detail")
        .or_else(|| string(item, "operation"))
        .or_else(|| string(item, "title"))
        .or_else(|| string(item, "text"))
        .unwrap_or_default()
}

fn brief_item_id(item: &Value) -> String {
    string(item, "taskId")
        .or_else(|| string(item, "approvalId"))
        .unwrap_or_else(|| "-".to_owned())
}

fn render_task_message(value: &Value) -> String {
    let task_id = string(value, "taskId").unwrap_or_else(|| "?".to_owned());
    let outcome = string(value, "outcome").unwrap_or_else(|| "?".to_owned());
    if outcome == "queued" {
        let message_id = string(value, "messageId").unwrap_or_else(|| "?".to_owned());
        format!("{task_id}: message queued ({message_id})")
    } else {
        format!("{task_id}: message {outcome}")
    }
}

fn render_task_messages(value: &Value) -> String {
    let items = list_items(value);
    if items.is_empty() {
        return "(no messages)".to_owned();
    }
    items
        .iter()
        .map(|message| {
            let ts = string(message, "ts").unwrap_or_else(|| "-".to_owned());
            let direction = string(message, "direction").unwrap_or_else(|| "?".to_owned());
            let status = string(message, "status").unwrap_or_else(|| "?".to_owned());
            let text = string(message, "text").unwrap_or_default();
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let text = truncate_chars(&text, 120);
            format!("{ts} {direction} {status} {text}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_task_brief_global(value: &Value) -> String {
    let generated = string(value, "generatedAt").unwrap_or_else(|| "-".to_owned());
    let total = value
        .get("counts")
        .and_then(|counts| counts.get("total"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let needs_action = value
        .get("needsAction")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let running = value
        .get("running")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let done = value
        .get("done")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut lines = vec![format!(
        "Generated {generated} · {total} tasks · {} need action",
        needs_action.len()
    )];
    if !needs_action.is_empty() {
        lines.push(format!("{BOLD}Needs action{RESET}"));
        for item in &needs_action {
            let kind = string(item, "kind").unwrap_or_else(|| "?".to_owned());
            lines.push(format!(
                "{kind} · {} · {}",
                brief_item_id(item),
                brief_item_detail(item)
            ));
        }
    }
    if !running.is_empty() {
        lines.push(format!("{BOLD}Running{RESET}"));
        for item in &running {
            let task_id = brief_item_id(item);
            let title = string(item, "title").unwrap_or_default();
            let status = string(item, "status").unwrap_or_else(|| "?".to_owned());
            lines.push(format!("{task_id} · {title} · {status}"));
        }
    }
    if !done.is_empty() {
        lines.push(format!("{BOLD}Done{RESET}"));
        for item in &done {
            let task_id = brief_item_id(item);
            let title = string(item, "title").unwrap_or_default();
            let status = string(item, "status").unwrap_or_else(|| "?".to_owned());
            lines.push(format!("{task_id} · {title} · {status}"));
        }
    }
    lines.join("\n")
}

fn render_task_brief_task(value: &Value) -> String {
    let task_id = string(value, "taskId").unwrap_or_else(|| "?".to_owned());
    let title = string(value, "title").unwrap_or_default();
    let status = string(value, "status").unwrap_or_else(|| "?".to_owned());
    let project = string(value, "projectPath")
        .or_else(|| string(value, "projectId"))
        .unwrap_or_else(|| "-".to_owned());
    let ceo = value
        .get("ceo")
        .filter(|ceo| !ceo.is_null())
        .map(|ceo| {
            let id = string(ceo, "id").unwrap_or_else(|| "?".to_owned());
            let role = string(ceo, "role").unwrap_or_else(|| "?".to_owned());
            let ceo_status = string(ceo, "status").unwrap_or_else(|| "?".to_owned());
            format!("{id} · {role} · {ceo_status}")
        })
        .unwrap_or_else(|| "-".to_owned());
    let pending = value
        .get("pendingMessages")
        .and_then(Value::as_i64)
        .unwrap_or(0);

    let mut lines = vec![
        format!("{BOLD}{task_id} · {title}{RESET}"),
        format!("status   {status}"),
        format!("project  {project}"),
        format!("ceo      {ceo}"),
        format!("pending  {pending} message(s)"),
    ];

    if let Some(summary) = text_or_array(value.get("summary")) {
        lines.push(String::new());
        lines.push(format!("{BOLD}Summary{RESET}"));
        lines.push(summary);
    }
    let decisions = lines_or_array(value.get("decisions"));
    if !decisions.is_empty() {
        lines.push(String::new());
        lines.push(format!("{BOLD}Decisions{RESET}"));
        for decision in decisions {
            lines.push(format!("- {decision}"));
        }
    }
    let blockers = lines_or_array(value.get("blockers"));
    if !blockers.is_empty() {
        lines.push(String::new());
        lines.push(format!("{BOLD}Blockers{RESET}"));
        for blocker in blockers {
            lines.push(format!("- {blocker}"));
        }
    }
    if let Some(next_step) = text_or_array(value.get("nextStep")) {
        lines.push(String::new());
        lines.push(format!("{BOLD}Next step{RESET}"));
        lines.push(next_step);
    }
    lines.join("\n")
}

fn render_task_brief(value: &Value) -> String {
    if value.get("generatedAt").is_some() {
        render_task_brief_global(value)
    } else {
        render_task_brief_task(value)
    }
}

fn render_tasks_inbox(value: &Value) -> String {
    let items = list_items(value);
    if items.is_empty() {
        return "Inbox clear.".to_owned();
    }
    let mut lines: Vec<String> = Vec::new();
    for kind in ["approval", "blocked", "failed", "queued", "review"] {
        let group: Vec<&Value> = items
            .iter()
            .filter(|item| string(item, "kind").as_deref() == Some(kind))
            .collect();
        if group.is_empty() {
            continue;
        }
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(format!("== {kind} ({}) ==", group.len()));
        for item in group {
            let id = brief_item_id(item);
            let primary = string(item, "operation")
                .or_else(|| string(item, "title"))
                .or_else(|| string(item, "text"))
                .or_else(|| string(item, "detail"))
                .unwrap_or_default();
            let mut line = format!("{id} · {primary}");
            if let Some(detail) = string(item, "detail") {
                if detail != primary && !detail.is_empty() {
                    line.push_str(&format!(" · {detail}"));
                }
            }
            lines.push(line);
        }
    }
    lines.join("\n")
}

fn render_task_update(value: &Value) -> String {
    let task_id = string(value, "taskId").unwrap_or_else(|| "?".to_owned());
    let status = string(value, "status").unwrap_or_else(|| "?".to_owned());
    format!("{task_id} → {status}")
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
            let version = project
                .get("scaffoldVersion")
                .and_then(Value::as_str)
                .unwrap_or("—")
                .to_owned();
            let outdated = project
                .get("scaffoldOutdated")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let scaffold = if version == "—" {
                "— (not synced)".to_owned()
            } else if outdated {
                format!("{version} (stale)")
            } else {
                version
            };
            vec![
                string(project, "projectId").unwrap_or_else(|| "?".to_owned()),
                string(project, "name").unwrap_or_default(),
                scaffold,
                string(project, "path").unwrap_or_default(),
            ]
        })
        .collect();
    table(&["ID", "NAME", "SCAFFOLD", "PATH"], &rows)
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

fn render_approvals_list(value: &Value) -> String {
    let items = list_items(value);
    if items.is_empty() {
        return "No approvals.".to_owned();
    }
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            vec![
                string(item, "id").unwrap_or_else(|| "?".to_owned()),
                string(item, "status").unwrap_or_else(|| "?".to_owned()),
                string(item, "operation").unwrap_or_else(|| "-".to_owned()),
                truncate_chars(&string(item, "detail").unwrap_or_else(|| "-".to_owned()), 60),
                string(item, "createdAt").unwrap_or_else(|| "-".to_owned()),
            ]
        })
        .collect();
    table(&["ID", "STATUS", "OPERATION", "DETAIL", "CREATED"], &rows)
}

fn render_events(value: &Value) -> String {
    let items = list_items(value);
    if items.is_empty() {
        return "No events.".to_owned();
    }
    items
        .iter()
        .map(|item| {
            let ts = string(item, "ts").unwrap_or_else(|| "-".to_owned());
            let ts = ts.get(0..19).map(str::to_owned).unwrap_or(ts).replace('T', " ");
            let kind = string(item, "kind").unwrap_or_else(|| "?".to_owned());
            let message = truncate_chars(&string(item, "message").unwrap_or_default(), 110);
            format!("{DIM}{ts}{RESET}  {kind}  {message}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn session_short(session_id: Option<&str>) -> String {
    match session_id {
        Some(id) => {
            if id.chars().count() > 18 {
                let keep: String = id.chars().take(16).collect();
                format!("{keep}…")
            } else {
                id.to_owned()
            }
        }
        None => "-".to_owned(),
    }
}

fn render_agent_detail(value: &Value) -> String {
    let id = string(value, "id").unwrap_or_else(|| "?".to_owned());
    let role = string(value, "role").unwrap_or_else(|| "?".to_owned());
    let mode = string(value, "mode").unwrap_or_else(|| "?".to_owned());
    let project = match (string(value, "projectName"), string(value, "projectId")) {
        (Some(name), Some(id)) => format!("{name} ({id})"),
        (Some(name), None) => name,
        (None, Some(id)) => id,
        (None, None) => "-".to_owned(),
    };
    let live = value.get("live").filter(|live| !live.is_null());
    let live_text = match live {
        Some(live) => {
            if let Some(status) = live.get("status").and_then(Value::as_str) {
                format!(
                    "{} ({})",
                    live.get("state").and_then(Value::as_str).unwrap_or("?"),
                    status
                )
            } else {
                live.get("state")
                    .or_else(|| live.get("outcome"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned()
            }
        }
        None => "unreachable".to_owned(),
    };
    detail(&[
        ("agent", format!("{id} · {role} · {mode}")),
        ("project", project),
        ("status", string(value, "status").unwrap_or_else(|| "?".to_owned())),
        ("live", live_text),
        ("session", nullable(value, "sessionId")),
        ("terminal", nullable(value, "hcAddress")),
        ("directory", string(value, "directory").unwrap_or_else(|| "-".to_owned())),
        ("created", string(value, "createdAt").unwrap_or_else(|| "-".to_owned())),
        ("updated", string(value, "updatedAt").unwrap_or_else(|| "-".to_owned())),
    ])
}

fn render_agent_action(value: &Value, verb: &str) -> String {
    let id = string(value, "id")
        .or_else(|| string(value, "botId"))
        .unwrap_or_else(|| "?".to_owned());
    match verb {
        "stopped" => format!("{id} stopped (session kept)"),
        _ => {
            let turn = value.get("turn").and_then(Value::as_i64);
            match turn {
                Some(turn) => format!("prompt sent to {id} (turn {turn})"),
                None => format!("prompt sent to {id}"),
            }
        }
    }
}

fn render_agent_logs(value: &Value) -> String {
    let id = string(value, "id").or_else(|| string(value, "agentId")).unwrap_or_else(|| "?".to_owned());
    let status = string(value, "status").unwrap_or_else(|| "?".to_owned());
    let mode = string(value, "mode").unwrap_or_else(|| "?".to_owned());
    let session = string(value, "sessionId").unwrap_or_else(|| "-".to_owned());
    let mut out = format!("{BOLD}{id}{RESET}  {mode} · {status} · session {session}");
    if let Some(screen) = value.get("screen").and_then(Value::as_str) {
        let tail: Vec<&str> = screen.lines().rev().take(40).collect();
        out.push_str(&format!("\n\n{BOLD}SCREEN (last 40 lines){RESET}"));
        for line in tail.into_iter().rev() {
            out.push_str(&format!("\n{DIM}{line}{RESET}"));
        }
    }
    let messages = value.get("messages").and_then(Value::as_array).cloned().unwrap_or_default();
    out.push_str(&format!("\n\n{BOLD}MESSAGES ({}){RESET}", messages.len()));
    if messages.is_empty() {
        out.push_str("\n(no messages)");
        return out;
    }
    for message in &messages {
        let kind = string(message, "type").unwrap_or_else(|| "?".to_owned());
        let created = string(message, "createdAt")
            .map(|ts| ts.get(0..19).map(str::to_owned).unwrap_or(ts).replace('T', " "))
            .unwrap_or_else(|| "-".to_owned());
        let text = string(message, "text").unwrap_or_default();
        let text = truncate_chars(&text.replace('\n', " "), 220);
        out.push_str(&format!("\n{DIM}{created}{RESET}  {kind}  {text}"));
    }
    out
}

fn render_agent_interrupt(value: &Value) -> String {
    let id = string(value, "botId").unwrap_or_else(|| "?".to_owned());
    let outcome = string(value, "outcome").unwrap_or_else(|| "unknown".to_owned());
    let key = string(value, "key");
    match key {
        Some(key) => format!("interrupted {id} ({outcome}, key {key})"),
        None => format!("interrupted {id} ({outcome})"),
    }
}

fn render_managed_agents(managed: &[Value], show_all: bool) -> String {
    // Collapse stopped rows into a single summary line (mirrors `hc ls`),
    // unless --all is passed. Errors stay visible — they are actionable.
    let finished_count = managed
        .iter()
        .filter(|agent| string(agent, "status").as_deref() == Some("stopped"))
        .count();
    let rows: Vec<Vec<String>> = managed
        .iter()
        .filter(|agent| show_all || string(agent, "status").as_deref() != Some("stopped"))
        .map(|agent| {
            let runtime = agent
                .get("hcAddress")
                .and_then(Value::as_str)
                .or_else(|| agent.get("sessionId").and_then(Value::as_str))
                .map(|value| session_short(Some(value)))
                .unwrap_or_else(|| "-".to_owned());
            let live = agent
                .get("live")
                .filter(|live| !live.is_null())
                .and_then(|live| live.get("state").or_else(|| live.get("outcome")).and_then(Value::as_str))
                .unwrap_or("?");
            vec![
                string(agent, "id").unwrap_or_else(|| "?".to_owned()),
                agent
                    .get("projectName")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| "-".to_owned()),
                string(agent, "role").unwrap_or_else(|| "?".to_owned()),
                string(agent, "mode").unwrap_or_else(|| "?".to_owned()),
                string(agent, "status").unwrap_or_else(|| "?".to_owned()),
                live.to_owned(),
                runtime,
            ]
        })
        .collect();
    if rows.is_empty() && finished_count > 0 && !show_all {
        return format!("{finished_count} finished hidden (use --all to show)");
    }
    let mut out = format!(
        "{}",
        table(&["ID", "PROJECT", "ROLE", "MODE", "STATUS", "LIVE", "RUNTIME"], &rows)
    );
    if finished_count > 0 && !show_all {
        out.push_str(&format!("\n{DIM}{finished_count} finished hidden (use --all to show){RESET}"));
    }
    out
}

fn render_agents(value: &Value, show_finished: bool) -> String {
    use std::collections::HashMap;

    let mut lines: Vec<String> = Vec::new();

    let managed = value
        .get("managed")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !managed.is_empty() {
        lines.push(format!("{BOLD}Managed bots{RESET}"));
        lines.push(render_managed_agents(&managed, show_finished));
        lines.push(String::new());
    }

    let items = list_items(value);
    if items.is_empty() {
        lines.push("No registered agents. See `kbots project add`.".to_owned());
    } else {
        lines.push(format!("{BOLD}Registered agents{RESET}"));
        let mut group_order: Vec<String> = Vec::new();
        let mut groups: HashMap<String, Vec<&Value>> = HashMap::new();
        for item in &items {
            let key = item
                .get("projectPath")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_owned();
            if !groups.contains_key(&key) {
                group_order.push(key.clone());
                groups.insert(key.clone(), Vec::new());
            }
            if let Some(group) = groups.get_mut(&key) {
                group.push(item);
            }
        }
        for (index, path) in group_order.iter().enumerate() {
            if index > 0 {
                lines.push(String::new());
            }
            let group = groups[path].clone();
            let first = group[0];
            let registered = first.get("registered").and_then(Value::as_bool).unwrap_or(false);
            let name = first
                .get("projectName")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| base_name(path).to_owned());
            let suffix = if registered {
                string(first, "projectId").unwrap_or_else(|| "-".to_owned())
            } else {
                "unregistered".to_owned()
            };
            lines.push(format!("{BOLD}{name}{RESET}  {DIM}{suffix} · {path}{RESET}"));
            let rows: Vec<Vec<String>> = group
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
                        string(item, "role").unwrap_or_else(|| "?".to_owned()),
                        item.get("enabled").map(as_bool).unwrap_or_else(|| "?".to_owned()),
                        nullable(item, "sessionID"),
                        herdr_cell,
                    ]
                })
                .collect();
            lines.push(table(&["ROLE", "ENABLED", "SESSION", "HERDR"], &rows));
        }
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
        _ => {
            lines.push(String::new());
            let filtered = value.get("filteredOut").and_then(Value::as_i64).unwrap_or(0);
            if filtered > 0 {
                lines.push(format!(
                    "{DIM}Live terminals (hc): none for registered projects ({filtered} from other projects hidden){RESET}"
                ));
            } else {
                lines.push(format!("{DIM}Live terminals (hc): none{RESET}"));
            }
        }
    }
    lines.join("\n")
}

fn render_watchers_list(value: &Value) -> String {
    let items = list_items(value);
    if items.is_empty() {
        return "No watchers configured. See `kbots project watch --project <ref>` to enable one.".to_owned();
    }
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|watcher| {
            vec![
                watcher
                    .get("projectName")
                    .or_else(|| watcher.get("projectId"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| "?".to_owned()),
                if watcher.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
                    "on".to_owned()
                } else {
                    "off".to_owned()
                },
                watcher
                    .get("intervalSeconds")
                    .and_then(Value::as_u64)
                    .map(|value| format!("{value}s"))
                    .unwrap_or_else(|| "-".to_owned()),
                watcher
                    .get("debounceSeconds")
                    .and_then(Value::as_u64)
                    .map(|value| format!("{value}s"))
                    .unwrap_or_else(|| "-".to_owned()),
                watcher
                    .get("cooldownSeconds")
                    .and_then(Value::as_u64)
                    .map(|value| format!("{value}s"))
                    .unwrap_or_else(|| "-".to_owned()),
                nullable(watcher, "lastChangeAt"),
                nullable(watcher, "lastDeliveredAt"),
            ]
        })
        .collect();
    table(&["PROJECT", "STATE", "INTERVAL", "DEBOUNCE", "COOLDOWN", "LAST CHANGE", "LAST DELIVERED"], &rows)
}

fn render_watcher_set(value: &Value) -> String {
    let project = value
        .get("projectName")
        .or_else(|| value.get("projectId"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| "?".to_owned());
    let enabled = value.get("enabled").and_then(Value::as_bool).unwrap_or(false);
    let interval = value.get("intervalSeconds").and_then(Value::as_u64).unwrap_or(0);
    let debounce = value.get("debounceSeconds").and_then(Value::as_u64).unwrap_or(0);
    let cooldown = value.get("cooldownSeconds").and_then(Value::as_u64).unwrap_or(0);
    if enabled {
        format!("watcher enabled for {project} (interval {interval}s, debounce {debounce}s, cooldown {cooldown}s)")
    } else {
        format!("watcher disabled for {project}")
    }
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

fn render_project_sync(value: &Value) -> String {
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
    if value.get("check").and_then(Value::as_bool).unwrap_or(false) {
        let up_to_date = value.get("upToDate").and_then(Value::as_bool).unwrap_or(false);
        return detail(&[
            ("version", string(value, "kbotsVersion").unwrap_or_else(|| "?".to_owned())),
            (
                "recorded",
                string(value, "recordedVersion").unwrap_or_else(|| "never synced".to_owned()),
            ),
            ("state", if up_to_date { "up to date".to_owned() } else { "needs sync".to_owned() }),
            ("stale", format_list("stale")),
            ("customized", format_list("customized")),
        ]);
    }
    let format_list_original = |key: &str| {
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
        ("version", string(value, "kbotsVersion").unwrap_or_else(|| "?".to_owned())),
        ("previous", string(value, "previousVersion").unwrap_or_else(|| "-".to_owned())),
        ("created", format_list_original("created")),
        ("updated", format_list_original("updated")),
        ("current", format_list_original("current")),
        ("kept", format_list_original("kept")),
        ("backups", format_list_original("backups")),
        ("stamp", string(value, "stamp").unwrap_or_else(|| "-".to_owned())),
    ])
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
