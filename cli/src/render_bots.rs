//! Bot reporting for the human-readable CLI: the activity/health block that
//! sits under `kbots status`, `kbots agents`, and `kbots agents show`.
//!
//! The daemon derives every value (`src/backend/bots/health.ts`); this module
//! only formats it, so the CLI, TUI, and any other client agree on what a code
//! like `idle_with_open_task` means.
use serde_json::Value;

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const GREEN: &str = "\x1b[32m";
const RESET: &str = "\x1b[0m";

fn level_glyph(level: &str) -> (&'static str, &'static str) {
    match level {
        "error" => ("✖", RED),
        "warning" => ("⚠", YELLOW),
        "ok" => ("✔", GREEN),
        "waiting" => ("·", DIM),
        _ => ("·", DIM),
    }
}

fn report_of(item: &Value) -> Option<&Value> {
    item.get("report").filter(|value| value.is_object())
}

fn health_of(report: &Value) -> (&str, &str, &str, Option<&str>) {
    let health = report.get("health");
    let level = health
        .and_then(|value| value.get("level"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let code = health
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let message = health
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let hint = health
        .and_then(|value| value.get("hint"))
        .and_then(Value::as_str);
    (level, code, message, hint)
}

fn task_of(report: &Value) -> Option<(String, String)> {
    let task = report.get("task")?;
    let id = task.get("taskId").and_then(Value::as_str)?.to_string();
    let status = task
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string();
    Some((id, status))
}

fn activity_of(report: &Value) -> &str {
    report
        .get("activity")
        .and_then(|value| value.get("detail"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
}

/// `kbots status`: one summary line plus every problem, under the counters.
pub fn status_section(value: &Value) -> String {
    let Some(bots) = value.get("bots") else {
        return String::new();
    };
    let total = bots.get("total").and_then(Value::as_u64).unwrap_or(0);
    let by_health = bots.get("byHealth").cloned().unwrap_or(Value::Null);
    let count = |key: &str| {
        by_health
            .get(key)
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let problems = bots
        .get("problems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut out = String::new();
    out.push_str(&format!(
        "\nbots      {total} ({} ok, {} waiting, {} warning, {} error, {} stopped)\n",
        count("ok"),
        count("waiting"),
        count("warning"),
        count("error"),
        count("stopped"),
    ));
    for problem in problems.iter().take(10) {
        let level = problem.get("level").and_then(Value::as_str).unwrap_or("warning");
        let (glyph, color) = level_glyph(level);
        let agent = problem
            .get("agentId")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let role = problem.get("role").and_then(Value::as_str).unwrap_or("?");
        let task = problem
            .get("taskId")
            .and_then(Value::as_str)
            .map(|id| format!(" · {id}"))
            .unwrap_or_default();
        let message = problem
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("");
        out.push_str(&format!(
            "          {color}{glyph}{RESET} {BOLD}{agent}{RESET} {role}{task}: {message}\n"
        ));
    }
    out
}

/// `kbots agents`: a compact activity line per bot, plus the problems.
pub fn agents_section(value: &Value) -> String {
    let managed = value
        .get("managed")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if managed.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    out.push_str(&format!("\n{BOLD}bots{RESET}\n"));
    for agent in &managed {
        let Some(report) = report_of(agent) else {
            continue;
        };
        let (level, code, message, hint) = health_of(report);
        let id = agent.get("id").and_then(Value::as_str).unwrap_or("?");
        let role = agent.get("role").and_then(Value::as_str).unwrap_or("?");
        let (glyph, color) = level_glyph(level);
        let task = task_of(report)
            .map(|(id, status)| format!(" · {id} {status}"))
            .unwrap_or_else(|| " · no task".to_string());
        let detail = activity_of(report);
        out.push_str(&format!(
            "  {color}{glyph}{RESET} {BOLD}{id}{RESET} {role:<4}{task} {DIM}{detail}{RESET}\n"
        ));
        if message.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "      {color}{code}{RESET} {message}\n"
        ));
        if let Some(hint) = hint {
            out.push_str(&format!("      {DIM}→ {hint}{RESET}\n"));
        }
    }
    out
}

/// `kbots agents show`: the same facts as labelled lines under the summary.
pub fn agent_section(value: &Value) -> String {
    let Some(report) = report_of(value) else {
        return String::new();
    };
    let (level, code, message, hint) = health_of(report);
    let (glyph, color) = level_glyph(level);
    let activity = activity_of(report);

    let mut out = String::new();
    out.push_str(&format!("\n{BOLD}activity{RESET}  {activity}\n"));
    let task = task_of(report);
    if let Some((id, status)) = task {
        out.push_str(&format!("{BOLD}task{RESET}      {id} ({status})\n"));
    }
    if message.is_empty() {
        out.push_str(&format!("{BOLD}health{RESET}    {color}{glyph} {level}{RESET}\n"));
    } else {
        out.push_str(&format!(
            "{BOLD}health{RESET}    {color}{glyph} {level} · {code}{RESET}\n          {message}\n"
        ));
    }
    if let Some(hint) = hint {
        out.push_str(&format!("{BOLD}hint{RESET}      {DIM}{hint}{RESET}\n"));
    }
    out
}
