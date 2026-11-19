//! Minimal JSON-RPC 2.0 client for the kbots daemon, speaking
//! newline-delimited JSON over a Unix domain socket.
//!
//! One request per connection (the v1 protocol): write a single line, read a
//! single response line. Read and write timeouts are ~10 s so a wedged
//! daemon can never freeze the UI for longer than that.

use std::env;
use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Map, Value, json};

const IO_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Where the socket lives, in the contract's discovery order:
/// `KBOTS_SOCKET` → `$XDG_RUNTIME_DIR/kbots.sock` → `$HOME/.local/run/kbots.sock`.
/// Returns the first candidate that exists; otherwise the best guess so the
/// error message names a useful path.
pub fn discover_socket() -> Option<PathBuf> {
    if let Ok(explicit) = env::var("KBOTS_SOCKET") {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    let xdg = env::var("XDG_RUNTIME_DIR")
        .ok()
        .map(|dir| PathBuf::from(dir).join("kbots.sock"))
        .filter(|path| !path.as_os_str().is_empty());
    if let Some(path) = xdg.as_ref()
        && path.exists()
    {
        return Some(path.clone());
    }
    let home = env::var("HOME")
        .ok()
        .map(|dir| PathBuf::from(dir).join(".local/run/kbots.sock"));
    if let Some(path) = home.as_ref()
        && path.exists()
    {
        return Some(path.clone());
    }
    xdg.or(home).filter(|path| path.parent().is_some())
}

#[derive(Debug)]
pub enum RpcError {
    /// The socket could not be reached at all (daemon down, stale path).
    Connect(String),
    /// I/O failed after connecting, or the response could not be read.
    Transport(String),
    /// The response line was not valid JSON-RPC 2.0.
    Parse(String),
    /// The daemon answered with a JSON-RPC error object.
    Method { code: i64, message: String },
}

impl RpcError {
    pub fn is_method_not_found(&self) -> bool {
        matches!(self, RpcError::Method { code: -32601, .. })
    }

    /// True when the failure means "daemon unreachable", as opposed to a
    /// single failing method.
    pub fn is_unreachable(&self) -> bool {
        matches!(self, RpcError::Connect(_) | RpcError::Transport(_))
    }

    /// Short human-readable detail for inline display.
    pub fn detail(&self) -> String {
        match self {
            RpcError::Connect(msg) => format!("connect failed: {msg}"),
            RpcError::Transport(msg) => format!("socket error: {msg}"),
            RpcError::Parse(msg) => format!("bad response: {msg}"),
            RpcError::Method { code, message } => {
                if *code == -32601 {
                    "daemon not updated yet (method not found)".to_string()
                } else {
                    format!("daemon error {code}: {message}")
                }
            }
        }
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail())
    }
}

#[derive(Clone)]
pub struct RpcClient {
    socket_path: PathBuf,
}

impl RpcClient {
    pub fn new() -> Self {
        Self {
            socket_path: discover_socket().unwrap_or_else(|| PathBuf::from("kbots.sock")),
        }
    }

    /// Fire a single method call and return the raw `result` value.
    pub fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.call_with_timeout(method, params, IO_TIMEOUT)
    }

    fn call_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, RpcError> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .map_err(|error| RpcError::Connect(format!("{} ({})", self.socket_path.display(), error)))?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|error| RpcError::Transport(error.to_string()))?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|error| RpcError::Transport(error.to_string()))?;

        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut payload = request.to_string();
        payload.push('\n');
        stream
            .write_all(payload.as_bytes())
            .map_err(|error| RpcError::Transport(error.to_string()))?;

        let limited = stream.take(MAX_RESPONSE_BYTES);
        let mut line = String::new();
        BufReader::new(limited)
            .read_line(&mut line)
            .map_err(|error| RpcError::Transport(error.to_string()))?;
        if line.trim().is_empty() {
            return Err(RpcError::Parse("empty response line".into()));
        }
        let response: Value = serde_json::from_str(line.trim())
            .map_err(|error| RpcError::Parse(error.to_string()))?;

        if let Some(error) = response.get("error") {
            let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string();
            return Err(RpcError::Method { code, message });
        }
        response
            .get("result")
            .cloned()
            .ok_or_else(|| RpcError::Parse("response carries neither result nor error".into()))
    }

    fn call_typed<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T, RpcError> {
        let result = self.call(method, params)?;
        serde_json::from_value(result).map_err(|error| RpcError::Parse(error.to_string()))
    }

    pub fn tasks_list(&self) -> Result<Vec<Task>, RpcError> {
        #[derive(Deserialize)]
        struct ListResult {
            #[serde(default)]
            items: Vec<Task>,
        }
        Ok(self.call_typed::<ListResult>("tasks.list", json!({}))?.items)
    }

    pub fn tasks_create(&self, title: &str) -> Result<TaskCreateResult, RpcError> {
        self.call_typed("tasks.create", json!({ "title": title }))
    }

    pub fn tasks_message(&self, task_id: &str, text: &str) -> Result<TaskMessageResult, RpcError> {
        self.call_typed(
            "tasks.message",
            json!({ "taskId": task_id, "text": text }),
        )
    }

    pub fn tasks_update(
        &self,
        task_id: &str,
        status: &str,
        note: Option<&str>,
    ) -> Result<TaskUpdateResult, RpcError> {
        let mut params = json!({ "taskId": task_id, "status": status });
        if let Some(note) = note {
            params["note"] = json!(note);
        }
        self.call_typed("tasks.update", params)
    }

    /// Detail brief for a single task (`tasks.brief` with `taskId`).
    pub fn tasks_brief_task(&self, task_id: &str) -> Result<TaskBrief, RpcError> {
        self.call_typed("tasks.brief", json!({ "taskId": task_id }))
    }

    /// Queued/delivered message backlog for a task (`tasks.messages`).
    pub fn tasks_messages(&self, task_id: &str) -> Result<Vec<TaskMessage>, RpcError> {
        #[derive(Deserialize)]
        struct ListResult {
            #[serde(default)]
            items: Vec<TaskMessage>,
        }
        Ok(self
            .call_typed::<ListResult>("tasks.messages", json!({ "taskId": task_id }))?
            .items)
    }

    pub fn tasks_brief_global(&self) -> Result<Value, RpcError> {
        self.call("tasks.brief", json!({}))
    }

    pub fn tasks_inbox(&self) -> Result<Vec<InboxItem>, RpcError> {
        #[derive(Deserialize)]
        struct InboxResult {
            #[serde(default)]
            items: Vec<InboxItem>,
        }
        Ok(self.call_typed::<InboxResult>("tasks.inbox", json!({}))?.items)
    }

    pub fn agents_list(&self) -> Result<AgentsList, RpcError> {
        self.call_typed("agents.list", json!({}))
    }

    pub fn approvals_list_pending(&self) -> Result<Vec<Approval>, RpcError> {
        #[derive(Deserialize)]
        struct ListResult {
            #[serde(default)]
            items: Vec<Approval>,
        }
        Ok(self
            .call_typed::<ListResult>("approvals.list", json!({ "status": "pending" }))?
            .items)
    }

    pub fn events_list(&self, limit: u32) -> Result<Vec<EventItem>, RpcError> {
        #[derive(Deserialize)]
        struct ListResult {
            #[serde(default)]
            items: Vec<EventItem>,
        }
        Ok(self
            .call_typed::<ListResult>("events.list", json!({ "limit": limit }))?
            .items)
    }

    pub fn system_status(&self) -> Result<SystemStatus, RpcError> {
        self.call_typed("system.status", json!({}))
    }

}

// ---------------------------------------------------------------------------
// Typed result shapes (camelCase on the wire, matching the v1/v2 contract).
// All fields default so a partially-populated daemon response never fails.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    #[serde(default)]
    pub task_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub project_path: Option<String>,
    /// The v2 task payload may carry the bound CEO's agent id; v1 does not.
    #[serde(default)]
    pub ceo_agent_id: Option<String>,
}

impl Task {
    /// Short display name for the owning project: last path segment when the
    /// daemon gives us a path, else the raw project id.
    pub fn project_label(&self) -> String {
        if let Some(path) = self.project_path.as_deref().filter(|p| !p.is_empty())
            && let Some(name) = path.rsplit('/').find(|segment| !segment.is_empty())
        {
            return name.to_string();
        }
        self.project_id
            .clone()
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| "—".to_string())
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCreateResult {
    #[serde(default)]
    pub task_id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub ceo_agent_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskMessageResult {
    #[serde(default)]
    pub task_id: String,
    #[serde(default)]
    pub outcome: String,
    #[serde(default)]
    pub message_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskUpdateResult {
    #[serde(default)]
    pub task_id: String,
    #[serde(default)]
    pub status: String,
}

/// `decisions`/`blockers` may arrive as an array (backend splits newlines
/// before sending) or as a raw newline-separated string.
fn string_or_seq<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(match value {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Value::String(raw) => raw
            .split('\n')
            .map(|line| line.trim_end().to_string())
            .filter(|line| !line.is_empty())
            .collect(),
        _ => Vec::new(),
    })
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskBrief {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default, deserialize_with = "string_or_seq")]
    pub decisions: Vec<String>,
    #[serde(default, deserialize_with = "string_or_seq")]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub next_step: Option<String>,
    #[serde(default)]
    pub ceo: Option<TaskBriefCeo>,
    #[serde(default)]
    pub pending_messages: Option<u64>,
}

/// The CEO link embedded in `tasks.brief {taskId}`. `lastOutcome`/`lastOutcomeAt`
/// may appear in a future backend (v5); everything is optional.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskBriefCeo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub last_outcome: Option<String>,
    #[serde(default)]
    pub last_outcome_at: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskMessage {
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub direction: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    #[serde(default)]
    pub approval_id: String,
    #[serde(default)]
    pub operation: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventItem {
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentItem {
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub project_path: String,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub herdr: Option<HerdrAttach>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrAttach {
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedAgent {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub last_error: Option<String>,
    /// Raw probe result: `{state, status, turn, …}` for terminal mode,
    /// `{outcome, …}` for headless. Rendered best-effort.
    #[serde(default)]
    pub live: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentsList {
    #[serde(default)]
    pub items: Vec<AgentItem>,
    #[serde(default)]
    pub live_terminals: Vec<Value>,
    #[serde(default)]
    pub filtered_out: u64,
    #[serde(default)]
    pub registered_projects: u64,
    #[serde(default)]
    pub managed: Vec<ManagedAgent>,
    #[serde(default)]
    pub herdr_unavailable: Option<String>,
}

impl AgentsList {
    pub fn live_terminal_count(&self) -> usize {
        self.live_terminals.len()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonInfo {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub uptime_seconds: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemStatus {
    #[serde(default)]
    pub daemon: Option<DaemonInfo>,
    #[serde(default)]
    pub projects: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl InboxItem {
    /// Best-effort one-line summary for the inbox overlay.
    pub fn summary(&self) -> String {
        for key in [
            "title", "summary", "text", "detail", "message", "operation", "note", "reason",
        ] {
            if let Some(value) = self.extra.get(key) {
                let rendered = render_value(value);
                if !rendered.is_empty() {
                    return rendered;
                }
            }
        }
        if let Some(agent) = self.extra.get("ceoAgentId").and_then(Value::as_str) {
            return format!("ceo {agent}");
        }
        // Fall back to a compact rendering of whatever the daemon sent.
        let compact = Value::Object(self.extra.clone()).to_string();
        truncate_chars(&compact, 90)
    }
}

/// Render any JSON value as a single short line (used by brief/inbox overlays).
pub fn render_value(value: &Value) -> String {
    match value {
        Value::String(text) => truncate_chars(text, 120),
        Value::Number(_) | Value::Bool(_) => value.to_string(),
        Value::Null => String::new(),
        other => truncate_chars(&other.to_string(), 120),
    }
}

/// Extract a "task-ish" line from a brief list entry (needsAction/running/done).
pub fn brief_entry_line(value: &Value) -> String {
    if let Value::String(text) = value {
        return truncate_chars(text, 120);
    }
    let Some(object) = value.as_object() else {
        return render_value(value);
    };
    let id = ["taskId", "id", "task_id"]
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str));
    let title = ["title", "summary", "name", "detail", "text", "message"]
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str));
    let status = ["status", "kind"]
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str));
    match (id, title) {
        (Some(id), Some(title)) => match status {
            Some(status) => format!("{id} · {title} [{status}]"),
            None => format!("{id} · {title}"),
        },
        (Some(id), None) => match status {
            Some(status) => format!("{id} [{status}]"),
            None => id.to_string(),
        },
        (None, Some(title)) => title.to_string(),
        (None, None) => truncate_chars(&value.to_string(), 120),
    }
}

pub fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}
