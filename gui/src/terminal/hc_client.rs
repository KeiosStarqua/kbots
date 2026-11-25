use kbots_ipc::Stream;
use serde_json::{Value, json};
use std::{
    env,
    io::{BufRead, BufReader, ErrorKind, Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use super::screen::{ScreenCell, new_parser, snapshot};

const DATA: u8 = 0;
const CREDIT: u8 = 1;
const CLOSE: u8 = 2;
const RESIZE: u8 = 3;
const FOCUS: u8 = 4;
const FACTS: u8 = 5;
const MAX_FRAME: usize = 16 * 1024 * 1024;
/// Internal sentinel: no frame within the 20 ms read window.
const READ_TIMEOUT: &str = "herdr read timeout";

#[derive(Clone, Debug)]
pub struct TerminalInfo {
    pub address: String,
    pub name: String,
    pub cwd: String,
    pub agent: String,
    pub status: String,
}

pub struct HcClient {
    writer: Arc<Mutex<Stream>>,
    next_id: u64,
    stream_commands: Option<mpsc::Sender<StreamCommand>>,
    observe: bool,
}

impl HcClient {
    pub fn connect() -> Result<Self, String> {
        let path = socket_path()?;
        let mut stream = Stream::connect_unix_or_host(&path).map_err(|error| {
            format!(
                "Cannot connect to Herdr Core at {}: {error}",
                path.display()
            )
        })?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;
        stream.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":1,"method":"hello","params":{"generation":1,"framed":true}})).as_bytes()).map_err(|e| e.to_string())?;
        let mut line = String::new();
        BufReader::new(stream.try_clone().map_err(|e| e.to_string())?)
            .read_line(&mut line)
            .map_err(|e| format!("Herdr handshake failed: {e}"))?;
        let hello: Value = serde_json::from_str(&line)
            .map_err(|e| format!("Invalid Herdr handshake response: {e}"))?;
        if let Some(error) = hello.get("error") {
            return Err(format!("Herdr handshake rejected: {error}"));
        }
        if hello.pointer("/result/framed").and_then(Value::as_bool) != Some(true) {
            return Err("Herdr Core did not enable framed streaming on this connection".into());
        }
        let writer = stream.try_clone().map_err(|e| e.to_string())?;
        Ok(Self {
            writer: Arc::new(Mutex::new(writer)),
            next_id: 2,
            stream_commands: None,
            observe: false,
        })
    }

    pub fn list(&mut self) -> Result<Vec<TerminalInfo>, String> {
        let result = self.request("terminal.list", json!({"all":false}))?;
        let terminals = result
            .get("terminals")
            .and_then(Value::as_array)
            .ok_or("Herdr terminal.list returned no terminals array")?;
        Ok(terminals
            .iter()
            .filter_map(|value| {
                Some(TerminalInfo {
                    address: value.get("address")?.as_str()?.to_owned(),
                    name: value
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("terminal")
                        .to_owned(),
                    cwd: value
                        .get("cwd")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    agent: value
                        .get("agent")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    status: value
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("live")
                        .to_owned(),
                })
            })
            .collect())
    }

    pub fn attach(
        &mut self,
        address: &str,
        cols: u16,
        rows: u16,
        observe: bool,
        events: mpsc::SyncSender<TerminalEvent>,
    ) -> Result<u32, String> {
        self.observe = observe;
        let result = self.request(
            "terminal.attach",
            json!({
                "terminal": address,
                "cols": cols,
                "rows": rows,
                "history": 1000,
                "observe": observe,
                "display": {"bg":"#101218","fg":"#e5e7eb","cursor":"#ffffff","cell":[9,18]}
            }),
        )?;
        let channel = result
            .get("channel")
            .and_then(Value::as_u64)
            .ok_or("Herdr attach response has no channel")? as u32;
        let address = address.to_owned();
        let mut reader = self
            .writer
            .lock()
            .map_err(|_| "Herdr socket lock was poisoned")?
            .try_clone()
            .map_err(|e| e.to_string())?;
        let writer = self.writer.clone();
        let (command_tx, command_rx) = mpsc::channel();
        self.stream_commands = Some(command_tx);
        thread::Builder::new()
            .name(format!("hc-terminal-{address}"))
            .spawn(move || {
                let mut parser = new_parser(cols, rows);
                if !observe {
                    let _ = write_frame(&writer, channel, FOCUS, &[1]);
                }
                let _ = write_frame(
                    &writer,
                    channel,
                    FACTS,
                    br##"{"bg":"#101218","fg":"#e5e7eb","cursor":"#ffffff","cell":[9,18]}"##,
                );
                let _ = events.send(TerminalEvent::Attached {
                    address: address.clone(),
                    channel,
                });
                // A short read timeout replaces libc::poll(): it behaves the
                // same on Unix and works on Windows, where AF_UNIX does not.
                let _ = reader.set_read_timeout(Some(Duration::from_millis(20)));
                loop {
                        match read_frame(&mut reader) {
                            Ok(Some(frame)) => {
                                if frame.channel == channel && frame.kind == DATA {
                                    parser.process(&frame.payload);
                                    let cells = snapshot(&parser);
                                    if events
                                        .send(TerminalEvent::Screen {
                                            address: address.clone(),
                                            channel,
                                            cells,
                                        })
                                        .is_err()
                                    {
                                        return;
                                    }
                                    let credit = (frame.payload.len() as u32).to_le_bytes();
                                    if write_frame(&writer, channel, CREDIT, &credit).is_err() {
                                        return;
                                    }
                                } else if frame.channel == channel
                                    && frame.kind == RESIZE
                                    && frame.payload.len() == 4
                                {
                                    let cols =
                                        u16::from_le_bytes([frame.payload[0], frame.payload[1]]);
                                    let rows =
                                        u16::from_le_bytes([frame.payload[2], frame.payload[3]]);
                                    parser.set_size(
                                        std::num::NonZeroU16::new(rows.max(1)).unwrap(),
                                        std::num::NonZeroU16::new(cols.max(1)).unwrap(),
                                    );
                                    let _ = events.send(TerminalEvent::Screen {
                                        address: address.clone(),
                                        channel,
                                        cells: snapshot(&parser),
                                    });
                                } else if frame.channel == channel && frame.kind == CLOSE {
                                    let reason =
                                        String::from_utf8_lossy(&frame.payload).into_owned();
                                    let _ = events.send(TerminalEvent::Closed {
                                        address: address.clone(),
                                        channel,
                                        reason,
                                    });
                                    return;
                                }
                            }
                            Ok(None) => break,
                            Err(error) if error == READ_TIMEOUT => {}
                            Err(error) => {
                                let _ = events.send(TerminalEvent::Error {
                                    address: address.clone(),
                                    channel,
                                    message: error,
                                });
                                break;
                            }
                        }
                    match command_rx.try_recv() {
                        Ok(StreamCommand::Resize(cols, rows)) => {
                            parser.set_size(
                                std::num::NonZeroU16::new(rows.max(1)).unwrap(),
                                std::num::NonZeroU16::new(cols.max(1)).unwrap(),
                            );
                            let _ = events.send(TerminalEvent::Screen {
                                address: address.clone(),
                                channel,
                                cells: snapshot(&parser),
                            });
                        }
                        Ok(StreamCommand::Scroll(amount)) => {
                            let current = parser.screen().scrollback();
                            let target = if amount < 0 {
                                current.saturating_sub(amount.unsigned_abs() as usize)
                            } else {
                                current
                                    .saturating_add(amount as usize)
                                    .min(parser.screen().scrollback_len())
                            };
                            parser.screen_mut().set_scrollback(target);
                            let _ = events.send(TerminalEvent::Screen {
                                address: address.clone(),
                                channel,
                                cells: snapshot(&parser),
                            });
                        }
                        Err(mpsc::TryRecvError::Disconnected) => break,
                        Err(mpsc::TryRecvError::Empty) => {}
                    }
                }
            })
            .map_err(|e| format!("Could not start terminal stream: {e}"))?;
        Ok(channel)
    }

    pub fn send_input(&self, channel: u32, bytes: &[u8]) -> Result<(), String> {
        if self.observe {
            return Ok(());
        }
        write_frame(&self.writer, channel, DATA, bytes)
    }

    pub fn resize(&self, channel: u32, cols: u16, rows: u16) -> Result<(), String> {
        if self.observe {
            return Ok(());
        }
        let mut payload = cols.to_le_bytes().to_vec();
        payload.extend_from_slice(&rows.to_le_bytes());
        write_frame(&self.writer, channel, RESIZE, &payload)?;
        if let Some(commands) = &self.stream_commands {
            let _ = commands.send(StreamCommand::Resize(cols, rows));
        }
        Ok(())
    }

    pub fn focus(&self, channel: u32, focused: bool) -> Result<(), String> {
        if self.observe {
            return Ok(());
        }
        write_frame(&self.writer, channel, FOCUS, &[u8::from(focused)])
    }

    pub fn scrollback(&self, rows: i32) {
        if let Some(commands) = &self.stream_commands {
            let _ = commands.send(StreamCommand::Scroll(rows));
        }
    }

    pub fn detach(&self, channel: u32) {
        let _ = write_frame(&self.writer, channel, CLOSE, &[]);
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        write_frame(&self.writer, 0, DATA, request.to_string().as_bytes())?;
        let mut reader = self
            .writer
            .lock()
            .map_err(|_| "Herdr socket lock was poisoned")?
            .try_clone()
            .map_err(|e| e.to_string())?;
        reader
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;
        let result = (|| loop {
            let frame = read_frame(&mut reader)?.ok_or("Herdr Core closed the connection")?;
            if frame.channel != 0 || frame.kind != DATA {
                continue;
            }
            let response: Value = serde_json::from_slice(&frame.payload)
                .map_err(|e| format!("Invalid Herdr JSON-RPC response: {e}"))?;
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = response.get("error") {
                return Err(format!("Herdr {method} failed: {error}"));
            }
            return response
                .get("result")
                .cloned()
                .ok_or_else(|| format!("Herdr {method} response has no result"));
        })();
        let _ = reader.set_read_timeout(None);
        result
    }
}

#[derive(Debug)]
pub enum TerminalEvent {
    Attached {
        address: String,
        channel: u32,
    },
    Screen {
        address: String,
        channel: u32,
        cells: Vec<ScreenCell>,
    },
    Closed {
        address: String,
        channel: u32,
        reason: String,
    },
    Error {
        address: String,
        channel: u32,
        message: String,
    },
}

enum StreamCommand {
    Resize(u16, u16),
    Scroll(i32),
}

struct Frame {
    channel: u32,
    kind: u8,
    payload: Vec<u8>,
}

fn read_frame(reader: &mut impl Read) -> Result<Option<Frame>, String> {
    let mut header = [0; 9];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
            ) =>
        {
            return Ok(None);
        }
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            return Err(READ_TIMEOUT.into());
        }
        Err(error) => return Err(format!("Herdr frame read failed: {error}")),
    }
    let channel = u32::from_le_bytes(header[..4].try_into().unwrap());
    let kind = header[4];
    let len = u32::from_le_bytes(header[5..9].try_into().unwrap()) as usize;
    if len > MAX_FRAME {
        return Err(format!("Herdr frame too large: {len} bytes"));
    }
    let mut payload = vec![0; len];
    reader
        .read_exact(&mut payload)
        .map_err(|e| {
            if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) {
                READ_TIMEOUT.into()
            } else {
                format!("Herdr frame payload read failed: {e}")
            }
        })?;
    Ok(Some(Frame {
        channel,
        kind,
        payload,
    }))
}

fn write_frame(
    writer: &Arc<Mutex<Stream>>,
    channel: u32,
    kind: u8,
    payload: &[u8],
) -> Result<(), String> {
    if payload.len() > MAX_FRAME {
        return Err("Herdr frame exceeds 16 MiB".into());
    }
    let mut stream = writer
        .lock()
        .map_err(|_| "Herdr socket lock was poisoned")?;
    stream
        .write_all(&channel.to_le_bytes())
        .and_then(|_| stream.write_all(&[kind]))
        .and_then(|_| stream.write_all(&(payload.len() as u32).to_le_bytes()))
        .and_then(|_| stream.write_all(payload))
        .map_err(|e| format!("Herdr frame write failed: {e}"))
}

fn socket_path() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("HC_SOCKET") {
        return Ok(PathBuf::from(path));
    }
    #[cfg(unix)]
    {
        if let Some(dir) = env::var_os("XDG_RUNTIME_DIR") {
            return Ok(PathBuf::from(dir).join("herdr-core/default.sock"));
        }
        return Ok(PathBuf::from(format!("/run/user/{}/herdr-core/default.sock", unsafe {
            libc::getuid()
        })));
    }
    #[cfg(windows)]
    {
        Err("Herdr Core is not available on Windows by default; set HC_SOCKET=tcp://HOST:PORT".into())
    }
}
