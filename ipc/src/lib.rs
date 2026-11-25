//! Shared kbots IPC transport.
//!
//! One newline-delimited JSON-RPC 2.0 request and response per connection.
//! Unix domain sockets stay the default on Unix (backward compatible); TCP on
//! loopback is the default on Windows and the opt-in remote transport on every
//! platform. TCP connections carry a shared secret from `KBOTS_TOKEN_FILE` so
//! a local port cannot be driven by an unrelated process.
//!
//! Endpoint discovery is shared by CLI, TUI and GUI so the three clients never
//! disagree about where the daemon lives.

use std::{
    env, fmt, fs,
    io::{self, BufRead, BufReader, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use serde_json::{Value, json};

/// Loopback address used when no remote host is configured.
pub const DEFAULT_TCP_ADDR: &str = "127.0.0.1:7878";
/// Response/request idle timeout.
pub const IO_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A daemon location. `Tcp` is the only transport on Windows; Unix keeps the
/// socket candidates from `docs/kbots-ipc.md` plus optional TCP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Endpoint {
    #[cfg(unix)]
    Unix(PathBuf),
    Tcp(String),
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(unix)]
            Endpoint::Unix(path) => write!(f, "{}", path.display()),
            Endpoint::Tcp(host) => write!(f, "{host}"),
        }
    }
}

fn nonempty_env(name: &str) -> Option<String> {
    env::var(name).ok().map(|value| value.trim().to_owned()).filter(|value| !value.is_empty())
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// State directory shared with the Bun daemon (same path on all platforms).
pub fn state_dir() -> PathBuf {
    home_dir().join(".local").join("state").join("kbots")
}

/// Token file written by the daemon on startup and read by every client.
pub fn token_file() -> PathBuf {
    env::var_os("KBOTS_TOKEN_FILE").map(PathBuf::from).unwrap_or_else(|| state_dir().join("token"))
}

/// Client config: `saved_host()` is set by `kbots connect --host HOST:PORT`.
pub fn client_config() -> PathBuf {
    state_dir().join("client.json")
}

/// Read the shared secret. `KBOTS_TOKEN` overrides the file (handy for remote
/// clients that cannot read the daemon's home directory).
pub fn read_token() -> Option<String> {
    nonempty_env("KBOTS_TOKEN").or_else(|| {
        fs::read_to_string(token_file()).ok().map(|text| text.trim().to_owned()).filter(|token| !token.is_empty())
    })
}

/// Host persisted by `kbots connect`. Unreadable or malformed config is inert.
pub fn saved_host() -> Option<String> {
    let text = fs::read_to_string(client_config()).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value.get("host").and_then(Value::as_str).map(str::to_owned).filter(|host| !host.is_empty())
}

fn client_config_value() -> Value {
    fs::read_to_string(client_config())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(|value: &Value| value.is_object())
        .unwrap_or_else(|| json!({}))
}

fn write_client_config(value: &Value) -> io::Result<()> {
    let path = client_config();
    fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    fs::write(&path, format!("{value}\n"))
}

/// Persist (`Some`) or clear (`None`) the remote daemon host.
pub fn save_host(host: Option<&str>) -> io::Result<()> {
    let mut config = client_config_value();
    match host {
        Some(host) if !host.trim().is_empty() => config["host"] = json!(host.trim()),
        _ => {
            if let Some(object) = config.as_object_mut() {
                object.remove("host");
            }
        }
    };
    write_client_config(&config)
}

/// Repo root recorded by the installer so `kbots daemon start` works from any
/// directory (a portable client has no systemd unit and no fixed cwd).
pub fn repo_root() -> Option<String> {
    env::var_os("KBOTS_REPO_ROOT")
        .map(|root| root.to_string_lossy().into_owned())
        .or_else(|| {
            client_config_value()
                .get("repoRoot")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .filter(|root| !root.is_empty())
}

pub fn save_repo_root(root: &str) -> io::Result<()> {
    let mut config = client_config_value();
    config["repoRoot"] = json!(root);
    write_client_config(&config)
}

/// Discovery order for all three clients. Explicit flags beat saved config;
/// saved config beats the platform default, which is how remote mode works
/// without passing `--host` to every command.
pub fn endpoints(explicit_socket: Option<PathBuf>, explicit_host: Option<String>) -> Vec<Endpoint> {
    if let Some(host) = explicit_host.or_else(|| nonempty_env("KBOTS_HOST")).or_else(|| nonempty_env("KBOTS_TCP")) {
        return vec![Endpoint::Tcp(host)];
    }
    let socket_override = explicit_socket
        .or_else(|| nonempty_env("KBOTS_SOCKET").map(PathBuf::from))
        .filter(|path| !path.as_os_str().is_empty());
    #[cfg(unix)]
    if let Some(path) = socket_override {
        return vec![Endpoint::Unix(path)];
    }
    #[cfg(windows)]
    let _ = socket_override; // AF_UNIX paths are not a daemon transport on Windows.
    if let Some(host) = saved_host() {
        return vec![Endpoint::Tcp(host)];
    }
    #[cfg(windows)]
    {
        return vec![Endpoint::Tcp(nonempty_env("KBOTS_ADDR").unwrap_or_else(|| DEFAULT_TCP_ADDR.to_owned()))];
    }
    #[cfg(unix)]
    {
        let mut candidates = Vec::new();
        if let Some(dir) = nonempty_env("XDG_RUNTIME_DIR") {
            candidates.push(Endpoint::Unix(PathBuf::from(dir).join("kbots.sock")));
        }
        let home = home_dir().join(".local").join("run").join("kbots.sock");
        if !candidates.contains(&Endpoint::Unix(home.clone())) {
            candidates.push(Endpoint::Unix(home));
        }
        candidates
    }
}

/// A stream that can be a Unix socket or a TCP connection. Both carry the same
/// newline-delimited JSON-RPC frames, so callers never branch on transport.
pub enum Stream {
    #[cfg(unix)]
    Unix(UnixStream),
    Tcp(TcpStream),
}

#[cfg(unix)]
use std::os::unix::net::UnixStream;

fn connect_tcp(host: &str, timeout: Duration) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, format!("no address for {host}"));
    for address in host.to_socket_addrs()? {
        match TcpStream::connect_timeout(&address, timeout) {
            Ok(stream) => return Ok(stream),
            Err(error) => last = error,
        }
    }
    Err(last)
}

impl Stream {
    pub fn connect(endpoint: &Endpoint) -> io::Result<Self> {
        match endpoint {
            #[cfg(unix)]
            Endpoint::Unix(path) => UnixStream::connect(path).map(Self::Unix),
            Endpoint::Tcp(host) => connect_tcp(host, CONNECT_TIMEOUT).map(Self::Tcp),
        }
    }

    /// Herdr Core is reached over a Unix socket path, or `tcp://HOST:PORT`
    /// (the only usable form on Windows).
    pub fn connect_unix_or_host(spec: &Path) -> io::Result<Self> {
        let text = spec.to_string_lossy();
        if let Some(host) = text.strip_prefix("tcp://") {
            return connect_tcp(host, CONNECT_TIMEOUT).map(Self::Tcp);
        }
        #[cfg(unix)]
        {
            return UnixStream::connect(spec).map(Self::Unix);
        }
        #[cfg(windows)]
        {
            let _ = text;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "on Windows set HC_SOCKET to tcp://HOST:PORT for Herdr Core",
            ))
        }
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        match self {
            #[cfg(unix)]
            Self::Unix(stream) => stream.try_clone().map(Self::Unix),
            Self::Tcp(stream) => stream.try_clone().map(Self::Tcp),
        }
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Self::Unix(stream) => stream.set_read_timeout(timeout),
            Self::Tcp(stream) => stream.set_read_timeout(timeout),
        }
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Self::Unix(stream) => stream.set_write_timeout(timeout),
            Self::Tcp(stream) => stream.set_write_timeout(timeout),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            #[cfg(unix)]
            Self::Unix(stream) => stream.read(buf),
            Self::Tcp(stream) => stream.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            #[cfg(unix)]
            Self::Unix(stream) => stream.write(buf),
            Self::Tcp(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Self::Unix(stream) => stream.flush(),
            Self::Tcp(stream) => stream.flush(),
        }
    }
}

#[derive(Debug)]
pub enum RpcError {
    /// No daemon endpoint could be reached.
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

    pub fn is_unreachable(&self) -> bool {
        matches!(self, RpcError::Connect(_) | RpcError::Transport(_))
    }

    pub fn detail(&self) -> String {
        match self {
            RpcError::Connect(msg) => format!("connect failed: {msg}"),
            RpcError::Transport(msg) => format!("transport error: {msg}"),
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

impl std::error::Error for RpcError {}

/// Fire one JSON-RPC method call at the first reachable endpoint.
pub fn call(endpoints: &[Endpoint], method: &str, params: Value) -> Result<Value, RpcError> {
    let mut stream = None;
    let mut errors = Vec::new();
    for endpoint in endpoints {
        match Stream::connect(endpoint) {
            Ok(connected) => {
                stream = Some(connected);
                break;
            }
            Err(error) => errors.push(format!("{endpoint} ({error})")),
        }
    }
    let mut stream = stream.ok_or_else(|| {
        RpcError::Connect(format!("cannot reach daemon (tried: {})", errors.join("; ")))
    })?;
    stream.set_read_timeout(Some(IO_TIMEOUT)).map_err(|e| RpcError::Transport(e.to_string()))?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).map_err(|e| RpcError::Transport(e.to_string()))?;

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut request = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    if let Some(token) = read_token() {
        request["token"] = json!(token);
    }
    let mut payload = request.to_string();
    payload.push('\n');
    stream.write_all(payload.as_bytes()).map_err(|e| RpcError::Transport(e.to_string()))?;

    let mut line = String::new();
    BufReader::new(Read::by_ref(&mut stream).take(MAX_RESPONSE_BYTES))
        .read_line(&mut line)
        .map_err(|e| RpcError::Transport(e.to_string()))?;
    if line.trim().is_empty() {
        return Err(RpcError::Parse("empty response line".into()));
    }
    let response: Value = serde_json::from_str(line.trim()).map_err(|e| RpcError::Parse(e.to_string()))?;
    if let Some(error) = response.get("error") {
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_owned();
        return Err(RpcError::Method { code, message });
    }
    response.get("result").cloned().ok_or_else(|| RpcError::Parse("response carries neither result nor error".into()))
}
