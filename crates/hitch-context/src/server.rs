//! The Hitch socket server (`docs/PROTOCOL.md`, "The Hitch socket").
//!
//! One thread accepts connections and one thread reads each connection.
//! Messages go to the engine through an [`InputSink`]; Hitch-to-client
//! messages go out through [`Clients`].

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use hitch_protocol::frame::{self, MAX_LINE_BYTES};
use hitch_protocol::{ClientMessage, PROTOCOL_VERSION, ServerMessage, Source};

use crate::engine::InputSink;

const WRITE_TIMEOUT: Duration = Duration::from_secs(1);

struct Conn {
    writer: UnixStream,
    capabilities: Vec<String>,
    /// Sources this connection reported and hasn't blurred.
    sources: HashSet<Source>,
    last_active: Instant,
}

/// The connected clients, for sending Hitch-to-client messages.
#[derive(Clone, Default)]
pub(crate) struct Clients {
    conns: Arc<Mutex<HashMap<u64, Conn>>>,
    /// Source sessions (the Zellij session) whose reports came with a
    /// `hello`, and that hello's capabilities. Kept after the connection
    /// closes: the Zellij plugin reports on one-shot connections.
    sessions: Arc<Mutex<HashMap<String, Vec<String>>>>,
}

impl Clients {
    fn lock(&self) -> MutexGuard<'_, HashMap<u64, Conn>> {
        self.conns.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Send `msg` to the most recently active client that listed
    /// `capability` in its `hello`. Returns false if there is none or the
    /// write failed.
    pub(crate) fn deliver(&self, capability: &str, msg: &ServerMessage) -> bool {
        let Ok(line) = frame::encode(msg) else {
            return false;
        };
        let mut conns = self.lock();
        let mut ids: Vec<(Instant, u64)> = conns
            .iter()
            .filter(|(_, c)| c.capabilities.iter().any(|c| c == capability))
            .map(|(id, c)| (c.last_active, *id))
            .collect();
        ids.sort_unstable_by(|a, b| b.cmp(a));
        for (_, id) in ids {
            let Some(c) = conns.get_mut(&id) else {
                continue;
            };
            if c.writer.write_all(line.as_bytes()).is_ok() {
                return true;
            }
            let _ = c.writer.shutdown(std::net::Shutdown::Both);
            conns.remove(&id);
        }
        false
    }

    /// Whether the client reporting `session` said it has `capability`.
    pub(crate) fn session_has(&self, session: &str, capability: &str) -> bool {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session)
            .is_some_and(|caps| caps.iter().any(|c| c == capability))
    }

    fn shutdown_all(&self) {
        for c in self.lock().values() {
            let _ = c.writer.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// A running server. Dropping it stops accepting, closes every connection
/// and removes the socket file.
pub(crate) struct Server {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    clients: Clients,
}

impl Server {
    /// Bind `path` and start serving. Fails with `AddrInUse` if another
    /// Hitch is already listening there.
    pub(crate) fn start(path: &Path, sink: InputSink) -> io::Result<Self> {
        let listener = bind(path)?;
        let stop = Arc::new(AtomicBool::new(false));
        let clients = Clients::default();
        let server = Self {
            path: path.to_path_buf(),
            stop: stop.clone(),
            clients: clients.clone(),
        };
        thread::Builder::new()
            .name("hitch-socket".into())
            .spawn(move || accept_loop(listener, stop, clients, sink))?;
        Ok(server)
    }

    pub(crate) fn clients(&self) -> &Clients {
        &self.clients
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept call.
        let _ = UnixStream::connect(&self.path);
        self.clients.shutdown_all();
        let _ = fs::remove_file(&self.path);
    }
}

fn bind(path: &Path) -> io::Result<UnixListener> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
        && !dir.exists()
    {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.file_type().is_socket() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} exists and is not a socket", path.display()),
            ));
        }
        if UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("Hitch is already running on {}", path.display()),
            ));
        }
        // Left over from a Hitch that didn't shut down cleanly.
        fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

fn accept_loop(listener: UnixListener, stop: Arc<AtomicBool>, clients: Clients, sink: InputSink) {
    let mut next_id = 0u64;
    for stream in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else {
            continue;
        };
        next_id += 1;
        let (id, clients, sink) = (next_id, clients.clone(), sink.clone());
        let _ = thread::Builder::new()
            .name("hitch-socket-conn".into())
            .spawn(move || {
                let _ = serve(id, stream, &clients, &sink);
                if let Some(conn) = clients.lock().remove(&id) {
                    // A long-lived plugin that went away without a blur:
                    // treat it as one.
                    for source in conn.sources {
                        sink.message(ClientMessage::Blur { source });
                    }
                }
            });
    }
}

/// Reads one connection's messages until it closes. Replies are best
/// effort: a one-shot client (`hitch send`, the Zellij plugin) closes as
/// soon as it has written, so the welcome to its `hello` can't be
/// delivered, and the messages after the hello must still be read.
fn serve(id: u64, stream: UnixStream, clients: &Clients, sink: &InputSink) -> io::Result<()> {
    // macOS refuses socket options with EINVAL once the peer has closed,
    // which a one-shot client like `hitch toggle` often has by now. Its
    // message is still there to read, so carry on without the timeout.
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = (&mut reader)
            .take(MAX_LINE_BYTES as u64)
            .read_until(b'\n', &mut buf)?;
        if n == 0 {
            return Ok(());
        }
        if buf.last() != Some(&b'\n') && n == MAX_LINE_BYTES {
            let _ = reply(clients, &mut writer, &error("line too long"));
            return Ok(());
        }
        let Ok(line) = std::str::from_utf8(&buf) else {
            let _ = reply(clients, &mut writer, &error("invalid UTF-8"));
            continue;
        };
        if line.trim().is_empty() {
            continue;
        }
        let msg = match frame::decode::<ClientMessage>(line) {
            Ok(m) => m,
            Err(e) => {
                let _ = reply(
                    clients,
                    &mut writer,
                    &error(&format!("invalid message: {e}")),
                );
                continue;
            }
        };
        match msg {
            ClientMessage::Hello {
                protocol,
                capabilities,
                ..
            } => {
                if protocol != PROTOCOL_VERSION {
                    let _ = reply(
                        clients,
                        &mut writer,
                        &error(&format!("unsupported protocol {protocol}")),
                    );
                    continue;
                }
                clients.lock().insert(
                    id,
                    Conn {
                        writer: writer.try_clone()?,
                        capabilities,
                        sources: HashSet::new(),
                        last_active: Instant::now(),
                    },
                );
                let _ = reply(
                    clients,
                    &mut writer,
                    &ServerMessage::Welcome {
                        protocol: PROTOCOL_VERSION,
                        version: env!("CARGO_PKG_VERSION").into(),
                    },
                );
            }
            ClientMessage::Context(ref r) => {
                let capabilities = clients.lock().get_mut(&id).map(|c| {
                    c.sources.insert(r.source.clone());
                    c.last_active = Instant::now();
                    c.capabilities.clone()
                });
                if let Some(session) = &r.session {
                    // A report without a hello forgets what the session
                    // could do: its plugin may have been unloaded.
                    clients
                        .sessions
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(session.clone(), capabilities.unwrap_or_default());
                }
                sink.message(msg);
            }
            ClientMessage::Blur { ref source } => {
                if let Some(c) = clients.lock().get_mut(&id) {
                    c.sources.remove(source);
                }
                sink.message(msg);
            }
            ClientMessage::Command { .. } => sink.message(msg),
            ClientMessage::Unknown => {}
        }
    }
}

fn error(message: &str) -> ServerMessage {
    ServerMessage::Error {
        message: message.into(),
    }
}

/// Write to this connection. Holds the clients lock so a concurrent
/// [`Clients::deliver`] can't interleave its line with ours.
fn reply(clients: &Clients, w: &mut UnixStream, msg: &ServerMessage) -> io::Result<()> {
    let line = frame::encode(msg).map_err(io::Error::other)?;
    let _guard = clients.lock();
    w.write_all(line.as_bytes())
}
