//! The clawswap context source (spec F3).
//!
//! clawswap's daemon knows which project's dev-server page was last in view
//! and where that project's source folder is. It streams this on
//! `GET /focus/events` as Server-Sent Events, one JSON object per event:
//!
//! ```text
//! data: {"project":"shop","root":"/home/u/code/shop","at":1762905600000}
//!
//! data: null
//! ```
//!
//! The first event is the focused project at the time of connecting (`null`
//! for none). After that clawswap sends one each time a page reports focus,
//! including the same project coming back into view, and a `: beat` comment
//! every 15 s.
//!
//! [`spawn`] follows that stream on a background thread and hands the
//! engine the same [`ClientMessage`]s a socket plugin would send:
//!
//! - a focused project: `Context` with `source: clawswap` and
//!   `project_root` set to its `root`, so its HITCH.md is shown;
//! - no focused project, or the daemon going away after one was focused:
//!   `Blur` for `clawswap`.
//!
//! Whether to prefer clawswap's report (only while the browser is frontmost)
//! is the resolver's call, not this module's.
//!
//! The daemon listens on `127.0.0.1:27390`, or `$CLAWSWAP_PORT`, the same
//! variable clawswap's own CLI reads. It answers only requests whose `Host`
//! is `127.0.0.1` or `localhost` and that carry no browser `Origin`. When it
//! isn't running (it exits once it has no projects), this source retries
//! with backoff from 1 s up to 30 s.
//!
//! [`projects`] asks the daemon once for the projects it runs, so the panel
//! can offer the branch page ([`page_url`]) of the one you're working in
//! ([`project_for`]).
//!
//! The client is plain `std` (one HTTP/1.1 request on a `TcpStream`, chunked
//! decoding, SSE parsing), so it adds no async runtime or HTTP crate.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::Deserialize;

use hitch_protocol::{ClientMessage, ContextReport, Source};

/// clawswap's default daemon port.
pub const DEFAULT_PORT: u16 = 27390;

/// Environment variable clawswap uses to pick the daemon's port.
pub const PORT_ENV: &str = "CLAWSWAP_PORT";

/// Path of the focus stream on the daemon.
pub const FOCUS_EVENTS_PATH: &str = "/focus/events";

/// Where to find the daemon and how patient to be with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClawswapConfig {
    /// The daemon's address. Only loopback makes sense: the daemon refuses
    /// any other `Host`.
    pub addr: SocketAddr,
    /// Give up on a connection that has said nothing (not even a beat) for
    /// this long, and reconnect. clawswap beats every 15 s.
    pub idle_timeout: Duration,
    /// First retry delay after a failed or dropped connection.
    pub min_backoff: Duration,
    /// Longest retry delay.
    pub max_backoff: Duration,
}

impl ClawswapConfig {
    /// The daemon on `127.0.0.1` at `port`.
    pub fn with_port(port: u16) -> Self {
        Self {
            addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)),
            idle_timeout: Duration::from_secs(45),
            min_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(30),
        }
    }

    /// The daemon at `$CLAWSWAP_PORT`, or 27390 when it is unset or invalid.
    pub fn from_env() -> Self {
        Self::with_port(port_from(std::env::var(PORT_ENV).ok().as_deref()))
    }
}

impl Default for ClawswapConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

fn port_from(value: Option<&str>) -> u16 {
    value
        .and_then(|v| v.trim().parse().ok())
        .filter(|&p| p != 0)
        .unwrap_or(DEFAULT_PORT)
}

/// One event from `/focus/events`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FocusEvent {
    /// clawswap's project id.
    pub project: String,
    /// The project's source folder, where its HITCH.md lives.
    pub root: PathBuf,
    /// When the page reported focus, ms since the Unix epoch.
    #[serde(default)]
    pub at: Option<i64>,
}

/// Parse one event's data: `Ok(None)` is clawswap's `null` (no focused
/// project).
pub fn parse_focus(data: &str) -> Result<Option<FocusEvent>, serde_json::Error> {
    serde_json::from_str(data)
}

/// Path of the daemon's project list.
pub const PROJECTS_PATH: &str = "/projects";

/// A project the daemon runs, from `GET /projects`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Project {
    /// clawswap's project id, its session's name. Its page is `/p/<id>`.
    pub id: String,
    /// The name its page shows.
    #[serde(default)]
    pub name: String,
    /// The checkout's root folder.
    pub root: PathBuf,
}

#[derive(Deserialize)]
struct ProjectList {
    projects: Vec<Project>,
}

/// The projects the daemon runs now. An error when it isn't running (the
/// connection is refused) or answers something else.
pub fn projects(config: &ClawswapConfig) -> io::Result<Vec<Project>> {
    let mut stream = TcpStream::connect_timeout(&config.addr, Duration::from_secs(1))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    write!(
        stream,
        "GET {PROJECTS_PATH} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Accept: application/json\r\n\
         User-Agent: hitch/{version}\r\n\
         Connection: close\r\n\r\n",
        port = config.addr.port(),
        version = env!("CARGO_PKG_VERSION"),
    )?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let chunked = read_head(&mut reader)?;
    let mut body = String::new();
    if chunked {
        Chunked::new(reader).read_to_string(&mut body)?;
    } else {
        reader.read_to_string(&mut body)?;
    }
    let list: ProjectList = serde_json::from_str(&body).map_err(io::Error::other)?;
    Ok(list.projects)
}

/// The project whose checkout holds one of `folders`: the deepest root
/// when checkouts are nested.
pub fn project_for<'a, 'p>(
    projects: &'p [Project],
    folders: impl IntoIterator<Item = &'a Path>,
) -> Option<&'p Project> {
    let folders: Vec<&Path> = folders.into_iter().collect();
    projects
        .iter()
        .filter(|p| !p.root.as_os_str().is_empty())
        .filter(|p| folders.iter().any(|f| f.starts_with(&p.root)))
        .max_by_key(|p| p.root.components().count())
}

/// The address of a project's branch page on the daemon.
pub fn page_url(config: &ClawswapConfig, id: &str) -> String {
    let mut path = String::new();
    for b in id.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            path.push(b as char);
        } else {
            path.push_str(&format!("%{b:02X}"));
        }
    }
    format!("http://127.0.0.1:{}/p/{path}", config.addr.port())
}

/// The message the engine gets for a focus event.
pub fn to_message(focus: Option<&FocusEvent>) -> ClientMessage {
    match focus {
        Some(f) => ClientMessage::Context(ContextReport {
            source: Source::Clawswap,
            project_root: Some(f.root.clone()),
            ..ContextReport::default()
        }),
        None => ClientMessage::Blur {
            source: Source::Clawswap,
        },
    }
}

/// A running clawswap source. Dropping it, or calling [`stop`](Self::stop),
/// ends the background thread.
pub struct ClawswapHandle {
    stop: Option<mpsc::Sender<()>>,
    stream: Arc<Mutex<Option<TcpStream>>>,
    thread: Option<JoinHandle<()>>,
}

impl ClawswapHandle {
    /// Stop following clawswap and wait for the thread to end. The sink gets
    /// nothing more.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        // Dropping the sender wakes a backoff wait; shutting the socket down
        // wakes a blocked read.
        self.stop.take();
        if let Some(s) = self.stream.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ClawswapHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Follow clawswap's focus stream on a background thread, calling `sink`
/// with a `Context` or `Blur` for source `clawswap` as the focused project
/// changes. See the module docs.
pub fn spawn<F>(config: ClawswapConfig, sink: F) -> ClawswapHandle
where
    F: FnMut(ClientMessage) + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    let stream = Arc::new(Mutex::new(None));
    let shared = Arc::clone(&stream);
    let thread = std::thread::Builder::new()
        .name("hitch-clawswap".into())
        .spawn(move || run(config, sink, rx, shared))
        .expect("spawn clawswap thread");
    ClawswapHandle {
        stop: Some(tx),
        stream,
        thread: Some(thread),
    }
}

fn run<F>(
    config: ClawswapConfig,
    mut sink: F,
    stop: mpsc::Receiver<()>,
    current: Arc<Mutex<Option<TcpStream>>>,
) where
    F: FnMut(ClientMessage),
{
    let stopped = |stop: &mpsc::Receiver<()>| {
        matches!(
            stop.try_recv(),
            Ok(()) | Err(mpsc::TryRecvError::Disconnected)
        )
    };
    let mut focused = false;
    let mut backoff = config.min_backoff;

    while !stopped(&stop) {
        let mut connected = false;
        let mut emit = |focus: Option<FocusEvent>| {
            // Repeat every focus (a page coming back into view counts), but
            // only blur when something was focused.
            if focus.is_some() || focused {
                focused = focus.is_some();
                sink(to_message(focus.as_ref()));
            }
        };
        let result = follow(&config, &current, &mut connected, &stop, &mut emit);
        if stopped(&stop) {
            return;
        }
        // A refused connection or a dropped stream both mean: try again
        // later. There is no log to report it to yet.
        let _ = result;
        // The daemon is gone or restarting: its focused project isn't
        // current any more.
        emit(None);
        if connected {
            backoff = config.min_backoff;
        }
        match stop.recv_timeout(backoff) {
            Err(RecvTimeoutError::Timeout) => {}
            _ => return,
        }
        backoff = (backoff * 2).min(config.max_backoff);
    }
}

/// One connection: request the stream and dispatch events until it ends.
fn follow(
    config: &ClawswapConfig,
    current: &Mutex<Option<TcpStream>>,
    connected: &mut bool,
    stop: &mpsc::Receiver<()>,
    emit: &mut dyn FnMut(Option<FocusEvent>),
) -> io::Result<()> {
    let mut stream = TcpStream::connect_timeout(&config.addr, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(config.idle_timeout))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    *current.lock().unwrap_or_else(|e| e.into_inner()) = Some(stream.try_clone()?);
    // The handle may have stopped between the check and storing the stream.
    if matches!(
        stop.try_recv(),
        Ok(()) | Err(mpsc::TryRecvError::Disconnected)
    ) {
        return Ok(());
    }

    write!(
        stream,
        "GET {FOCUS_EVENTS_PATH} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Accept: text/event-stream\r\n\
         Cache-Control: no-store\r\n\
         User-Agent: hitch/{version}\r\n\
         Connection: close\r\n\r\n",
        port = config.addr.port(),
        version = env!("CARGO_PKG_VERSION"),
    )?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let chunked = read_head(&mut reader)?;
    *connected = true;

    let body: Box<dyn BufRead> = if chunked {
        Box::new(BufReader::new(Chunked::new(reader)))
    } else {
        Box::new(reader)
    };
    let mut events = SseParser::default();
    for line in body.lines() {
        if let Some(data) = events.line(&line?) {
            // An event this version can't read is skipped, not fatal.
            if let Ok(focus) = parse_focus(&data) {
                emit(focus);
            }
        }
    }
    Ok(())
}

/// Read the status line and headers. Returns whether the body is chunked.
fn read_head(reader: &mut impl BufRead) -> io::Result<bool> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let status = line.split_whitespace().nth(1).unwrap_or_default();
    if status != "200" {
        return Err(io::Error::other(format!(
            "unexpected response {:?}",
            line.trim_end()
        )));
    }
    let mut chunked = false;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let header = line.trim_end();
        if header.is_empty() {
            return Ok(chunked);
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("transfer-encoding")
            && value.to_ascii_lowercase().contains("chunked")
        {
            chunked = true;
        }
    }
}

/// Decodes an HTTP/1.1 chunked body.
struct Chunked<R> {
    inner: R,
    /// Bytes left in the current chunk.
    left: usize,
    done: bool,
}

impl<R: BufRead> Chunked<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            left: 0,
            done: false,
        }
    }

    fn next_chunk(&mut self) -> io::Result<()> {
        let mut line = String::new();
        if self.inner.read_line(&mut line)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let size = line.trim().split(';').next().unwrap_or_default();
        self.left = usize::from_str_radix(size, 16)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad chunk size"))?;
        if self.left == 0 {
            self.done = true;
        }
        Ok(())
    }
}

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            self.next_chunk()?;
            if self.done {
                return Ok(0);
            }
        }
        let max = buf.len().min(self.left);
        let n = self.inner.read(&mut buf[..max])?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.left -= n;
        if self.left == 0 {
            // Each chunk ends with CRLF.
            let mut crlf = [0u8; 2];
            self.inner.read_exact(&mut crlf)?;
        }
        Ok(n)
    }
}

/// Server-Sent Events, just the parts clawswap uses: `data:` lines joined
/// with `\n` and dispatched on a blank line; comments and other fields
/// ignored.
#[derive(Debug, Default)]
struct SseParser {
    data: Option<String>,
}

impl SseParser {
    /// Feed one line (without its terminator). Returns an event's data when
    /// the line ends one.
    fn line(&mut self, line: &str) -> Option<String> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            return self.data.take();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        if field == "data" {
            let value = value.strip_prefix(' ').unwrap_or(value);
            match &mut self.data {
                Some(d) => {
                    d.push('\n');
                    d.push_str(value);
                }
                None => self.data = Some(value.to_owned()),
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Instant;

    fn event(project: &str, root: &str) -> String {
        format!(r#"{{"project":"{project}","root":"{root}","at":1762905600000}}"#)
    }

    fn context(root: &str) -> ClientMessage {
        ClientMessage::Context(ContextReport {
            source: Source::Clawswap,
            project_root: Some(root.into()),
            ..ContextReport::default()
        })
    }

    fn blur() -> ClientMessage {
        ClientMessage::Blur {
            source: Source::Clawswap,
        }
    }

    /// Writes `body` as a chunked SSE response, one chunk per part, the way
    /// Plug's `send_chunked` does.
    fn respond(stream: &mut TcpStream, parts: &[String]) {
        let mut req = BufReader::new(stream.try_clone().unwrap());
        let mut first = String::new();
        req.read_line(&mut first).unwrap();
        assert!(first.starts_with("GET /focus/events HTTP/1.1"), "{first}");
        let mut host = String::new();
        loop {
            let mut l = String::new();
            req.read_line(&mut l).unwrap();
            if l.to_ascii_lowercase().starts_with("host:") {
                host = l.clone();
            }
            if l.trim().is_empty() {
                break;
            }
        }
        assert!(host.contains("127.0.0.1"), "daemon needs a loopback Host");
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n"
        )
        .unwrap();
        for p in parts {
            write!(stream, "{:x}\r\n{p}\r\n", p.len()).unwrap();
        }
        stream.flush().unwrap();
    }

    fn config(listener: &TcpListener) -> ClawswapConfig {
        ClawswapConfig {
            min_backoff: Duration::from_millis(20),
            max_backoff: Duration::from_millis(50),
            ..ClawswapConfig::with_port(listener.local_addr().unwrap().port())
        }
    }

    fn collect(n: usize, rx: &mpsc::Receiver<ClientMessage>) -> Vec<ClientMessage> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = Vec::new();
        while got.len() < n {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(m) => got.push(m),
                Err(_) => panic!("expected {n} messages, got {got:?}"),
            }
        }
        got
    }

    #[test]
    fn port_defaults_and_env() {
        assert_eq!(port_from(None), DEFAULT_PORT);
        assert_eq!(port_from(Some("4000")), 4000);
        assert_eq!(port_from(Some("nope")), DEFAULT_PORT);
        assert_eq!(port_from(Some("0")), DEFAULT_PORT);
    }

    #[test]
    fn parses_focus_and_null() {
        let f = parse_focus(&event("shop", "/home/u/shop"))
            .unwrap()
            .unwrap();
        assert_eq!(f.project, "shop");
        assert_eq!(f.root, PathBuf::from("/home/u/shop"));
        assert_eq!(parse_focus("null").unwrap(), None);
        assert_eq!(to_message(Some(&f)), context("/home/u/shop"));
        assert_eq!(to_message(None), blur());
    }

    #[test]
    fn sse_parser_handles_comments_and_multiline() {
        let mut p = SseParser::default();
        assert_eq!(p.line(": beat"), None);
        assert_eq!(p.line(""), None, "a blank line with no data is no event");
        assert_eq!(p.line("data: a"), None);
        assert_eq!(p.line("data:b\r"), None);
        assert_eq!(p.line("id: 3"), None);
        assert_eq!(p.line(""), Some("a\nb".into()));
    }

    #[test]
    fn chunked_decoding() {
        let raw = b"5\r\nhello\r\n7;ext=1\r\n, world\r\n0\r\n\r\n";
        let mut out = String::new();
        Chunked::new(&raw[..]).read_to_string(&mut out).unwrap();
        assert_eq!(out, "hello, world");
    }

    #[test]
    fn follows_focus_changes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cfg = config(&listener);
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            respond(
                &mut s,
                &[
                    "data: null\n\n".into(),
                    format!("data: {}\n\n", event("a", "/code/a")),
                    ": beat\n\n".into(),
                    // An event split across chunks.
                    format!("data: {}", event("b", "/code/b")),
                    "\n\n".into(),
                    format!("data: {}\n\n", event("b", "/code/b")),
                    "data: null\n\n".into(),
                ],
            );
            // Keep the connection open until the test is done.
            std::thread::sleep(Duration::from_millis(300));
        });

        let (tx, rx) = mpsc::channel();
        let handle = spawn(cfg, move |m| tx.send(m).unwrap());
        let got = collect(4, &rx);
        assert_eq!(
            got,
            vec![
                context("/code/a"),
                context("/code/b"),
                // The same page coming back into view is reported again.
                context("/code/b"),
                blur(),
            ],
            "the leading null blurs nothing, since nothing was focused"
        );
        handle.stop();
        server.join().unwrap();
        assert!(rx.try_recv().is_err(), "nothing after stop");
    }

    #[test]
    fn blurs_when_daemon_goes_and_reconnects() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cfg = config(&listener);
        let server = std::thread::spawn(move || {
            // First daemon: one focus, then it exits.
            let (mut s, _) = listener.accept().unwrap();
            respond(&mut s, &[format!("data: {}\n\n", event("a", "/code/a"))]);
            drop(s);
            // A daemon that refuses (another app on the port) is retried too.
            let (mut s, _) = listener.accept().unwrap();
            write!(s, "HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\n\r\n").unwrap();
            drop(s);
            // Back again.
            let (mut s, _) = listener.accept().unwrap();
            respond(&mut s, &[format!("data: {}\n\n", event("c", "/code/c"))]);
            std::thread::sleep(Duration::from_millis(300));
        });

        let (tx, rx) = mpsc::channel();
        let handle = spawn(cfg, move |m| tx.send(m).unwrap());
        let got = collect(3, &rx);
        assert_eq!(got, vec![context("/code/a"), blur(), context("/code/c")]);
        drop(handle);
        server.join().unwrap();
    }

    /// Answers one request for `path` with `body` as JSON.
    fn serve_json(listener: TcpListener, path: &'static str, body: &'static str, chunked: bool) {
        let (mut stream, _) = listener.accept().unwrap();
        let mut req = BufReader::new(stream.try_clone().unwrap());
        let mut first = String::new();
        req.read_line(&mut first).unwrap();
        assert!(
            first.starts_with(&format!("GET {path} HTTP/1.1")),
            "{first}"
        );
        loop {
            let mut l = String::new();
            req.read_line(&mut l).unwrap();
            if l.trim().is_empty() {
                break;
            }
        }
        if chunked {
            let (a, b) = body.split_at(body.len() / 2);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n\
                 {:x}\r\n{a}\r\n{:x}\r\n{b}\r\n0\r\n\r\n",
                a.len(),
                b.len()
            )
            .unwrap();
        } else {
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    }

    const PROJECTS: &str = r#"{"projects":[{"id":"hitch","name":"hitch","root":"/code/hitch","port":27390},{"id":"shop","name":"shop","root":"/code/shop","port":27390}],"selected":"hitch","focused":null}"#;

    #[test]
    fn lists_projects() {
        for chunked in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let cfg = config(&listener);
            let server =
                std::thread::spawn(move || serve_json(listener, "/projects", PROJECTS, chunked));
            let got = projects(&cfg).unwrap();
            server.join().unwrap();
            assert_eq!(
                got,
                vec![
                    Project {
                        id: "hitch".into(),
                        name: "hitch".into(),
                        root: "/code/hitch".into()
                    },
                    Project {
                        id: "shop".into(),
                        name: "shop".into(),
                        root: "/code/shop".into()
                    },
                ]
            );
        }
    }

    #[test]
    fn no_daemon_is_an_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cfg = config(&listener);
        drop(listener);
        assert!(projects(&cfg).is_err());
    }

    #[test]
    fn finds_the_project_holding_a_folder() {
        let project = |id: &str, root: &str| Project {
            id: id.into(),
            name: id.into(),
            root: root.into(),
        };
        let list = [
            project("shop", "/code/shop"),
            project("api", "/code/shop/api"),
            project("hitch", "/code/hitch"),
        ];
        let id = |folders: &[&str]| {
            project_for(&list, folders.iter().map(Path::new)).map(|p| p.id.as_str())
        };
        assert_eq!(id(&["/code/hitch"]), Some("hitch"));
        assert_eq!(id(&["/code/hitch/crates/hitch-app"]), Some("hitch"));
        assert_eq!(id(&["/code/shop/api/lib"]), Some("api"));
        assert_eq!(id(&["/code/shop/web"]), Some("shop"));
        // A path component, not a string prefix.
        assert_eq!(id(&["/code/hitchhiker"]), None);
        assert_eq!(id(&[]), None);
    }

    #[test]
    fn page_urls() {
        let cfg = ClawswapConfig::with_port(27390);
        assert_eq!(page_url(&cfg, "hitch"), "http://127.0.0.1:27390/p/hitch");
        assert_eq!(
            page_url(&cfg, "a b/c"),
            "http://127.0.0.1:27390/p/a%20b%2Fc"
        );
    }

    #[test]
    fn stop_is_prompt_while_waiting_for_a_daemon() {
        // Nothing listens on this port once the listener is dropped.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cfg = ClawswapConfig {
            min_backoff: Duration::from_secs(30),
            ..config(&listener)
        };
        drop(listener);
        let handle = spawn(cfg, |m| panic!("unexpected {m:?}"));
        std::thread::sleep(Duration::from_millis(50));
        let start = Instant::now();
        handle.stop();
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn stop_is_prompt_while_streaming() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cfg = config(&listener);
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            respond(&mut s, &[format!("data: {}\n\n", event("a", "/code/a"))]);
            // Hold the stream open with no beats, like an idle daemon.
            let mut buf = [0u8; 1];
            let _ = s.read(&mut buf);
        });
        let (tx, rx) = mpsc::channel();
        let handle = spawn(cfg, move |m| {
            let _ = tx.send(m);
        });
        assert_eq!(collect(1, &rx), vec![context("/code/a")]);
        let start = Instant::now();
        handle.stop();
        assert!(start.elapsed() < Duration::from_secs(2));
        server.join().unwrap();
    }
}
