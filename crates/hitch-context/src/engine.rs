//! The engine: wires the sources to the resolver, debounces, and reports
//! to the app.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use hitch_protocol::{ClientMessage, Command, ContextReport, ServerMessage, Source, paths};

use crate::project::FsProjectFinder;
use crate::resolve::{AppTable, IgnoreList, Resolver, WindowFocus};
use crate::server::Server;
use crate::{ActiveContexts, DEBOUNCE};

/// What the engine tells the app. Delivered on the engine's thread.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    /// The current contexts changed. Debounced; never repeats the same set.
    Contexts(ActiveContexts),
    /// A command from the CLI or a shortcut (`hitch toggle`). Not debounced.
    Command(Command),
    /// Whether the GNOME Shell extension is on the bus. Sent at start and
    /// whenever it appears or leaves; onboarding offers to install it when
    /// it's missing.
    ShellExtension { available: bool },
    /// A Zed window showed Zed's default title, which has no file path, so
    /// Hitch can't tell the file. The app can suggest adding
    /// [`hitch_protocol::zed::RECOMMENDED_SETTINGS`] to Zed's settings. Sent at
    /// most once per run.
    ZedNeedsTitleSetting,
}

/// Engine settings.
#[derive(Debug, Clone)]
pub struct Config {
    /// Socket to listen on. `None`: `$HITCH_SOCKET`, else `~/.hitch/hitch.sock`.
    pub socket_path: Option<PathBuf>,
    /// Quiet time before a change is reported.
    pub debounce: Duration,
    /// Longest a change waits while inputs keep arriving.
    pub max_delay: Duration,
    pub ignore: IgnoreList,
    pub apps: AppTable,
    /// The notes file name (a user setting), used to find project folders.
    pub notes_file_name: String,
    /// Watch the GNOME Shell extension on D-Bus. Linux only.
    pub gnome_shell: bool,
    /// D-Bus address to use instead of the session bus (tests).
    pub dbus_address: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            socket_path: None,
            debounce: DEBOUNCE,
            max_delay: Duration::from_secs(1),
            ignore: IgnoreList::default(),
            apps: AppTable::default(),
            notes_file_name: hitch_store::NOTES_FILE_NAME.to_owned(),
            gnome_shell: cfg!(target_os = "linux"),
            dbus_address: None,
        }
    }
}

pub(crate) enum Input {
    Message(ClientMessage),
    Window(WindowFocus),
    WindowGone,
    Shell(bool),
    SetIgnore(IgnoreList),
    Stop,
}

/// Feeds the engine. Cheap to clone and send to another thread.
///
/// This is how a focus source plugs in: the socket server, the GNOME Shell
/// watcher, the clawswap client and later macOS detection all push into one
/// of these. A plugin-like source sends [`ClientMessage::Context`] and
/// [`ClientMessage::Blur`] exactly as it would over the socket; an OS focus
/// source sends [`InputSink::window_focus`].
#[derive(Clone)]
pub struct InputSink {
    tx: mpsc::Sender<Input>,
}

impl InputSink {
    /// A message as a socket client would send it.
    pub fn message(&self, msg: ClientMessage) {
        let _ = self.tx.send(Input::Message(msg));
    }

    /// The focused window changed.
    pub fn window_focus(&self, focus: WindowFocus) {
        let _ = self.tx.send(Input::Window(focus));
    }

    /// The OS focus source went away; plugin reports alone decide.
    pub fn window_gone(&self) {
        let _ = self.tx.send(Input::WindowGone);
    }

    pub(crate) fn shell_available(&self, available: bool) {
        let _ = self.tx.send(Input::Shell(available));
    }
}

struct Inner {
    sink: InputSink,
    current: Arc<Mutex<ActiveContexts>>,
    reports: Arc<Mutex<HashMap<Source, ContextReport>>>,
    server: Server,
    socket_path: PathBuf,
    #[cfg(target_os = "linux")]
    shell: Option<crate::gnome::Shell>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.sink.tx.send(Input::Stop);
    }
}

/// A running context engine. Clone it freely; it stops when the last clone
/// is dropped.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("socket_path", &self.inner.socket_path)
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// Start listening on the socket, watch the GNOME Shell extension, and
    /// call `on_event` (on the engine's own thread) as things change.
    ///
    /// Fails if the socket can't be bound, for example with `AddrInUse`
    /// when Hitch is already running. A missing session bus or extension is
    /// not an error; it shows up as `ShellExtension { available: false }`.
    pub fn start(config: Config, on_event: impl FnMut(Event) + Send + 'static) -> io::Result<Self> {
        let socket_path = match &config.socket_path {
            Some(p) => p.clone(),
            None => paths::socket_path()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?,
        };
        let (tx, rx) = mpsc::channel();
        let sink = InputSink { tx };
        let server = Server::start(&socket_path, sink.clone())?;

        let current = Arc::new(Mutex::new(ActiveContexts::default()));
        let reports = Arc::new(Mutex::new(HashMap::new()));
        let resolver = Resolver::new(
            Box::new(FsProjectFinder {
                notes_file_name: config.notes_file_name.clone(),
            }),
            config.apps.clone(),
            config.ignore.clone(),
        );
        let state = LoopState {
            resolver,
            current: current.clone(),
            reports: reports.clone(),
            debounce: config.debounce,
            max_delay: config.max_delay,
        };
        thread::Builder::new()
            .name("hitch-context".into())
            .spawn(move || state.run(rx, on_event))?;

        #[cfg(target_os = "linux")]
        let shell = if config.gnome_shell {
            match crate::gnome::Shell::start(config.dbus_address.as_deref(), sink.clone()) {
                Ok(s) => Some(s),
                Err(_) => {
                    sink.shell_available(false);
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            inner: Arc::new(Inner {
                sink,
                current,
                reports,
                server,
                socket_path,
                #[cfg(target_os = "linux")]
                shell,
            }),
        })
    }

    /// A handle for another focus source to feed the engine (clawswap).
    pub fn sink(&self) -> InputSink {
        self.inner.sink.clone()
    }

    /// The contexts as last reported in [`Event::Contexts`].
    pub fn current(&self) -> ActiveContexts {
        lock(&self.inner.current).clone()
    }

    /// The last report from `source`, kept after it blurs. The Zellij
    /// fallback uses its `session` to run `zellij --session <s> action
    /// write-chars` when no plugin is connected.
    pub fn last_report(&self, source: &Source) -> Option<ContextReport> {
        lock(&self.inner.reports).get(source).cloned()
    }

    /// Send `msg` to the most recently active connected client that has
    /// `capability` (e.g. `"write_chars"` for the Zellij plugin). Returns
    /// false if no such client is connected.
    pub fn deliver(&self, capability: &str, msg: &ServerMessage) -> bool {
        self.inner.server.clients().deliver(capability, msg)
    }

    /// Replace the ignore list (spec F10).
    pub fn set_ignore(&self, ignore: IgnoreList) {
        let _ = self.inner.sink.tx.send(Input::SetIgnore(ignore));
    }

    /// Pin or unpin: ask the GNOME Shell extension to keep Hitch's windows
    /// above others on every workspace. The pin holds while this engine
    /// lives, and is applied again if the extension restarts (screen lock).
    /// Fails with `Unsupported` when there is no session bus, and with an
    /// error from the call when the extension isn't on the bus (the pin is
    /// still applied once it appears).
    pub fn set_keep_above(&self, pinned: bool) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        if let Some(shell) = &self.inner.shell {
            return shell.set_keep_above(pinned).map_err(io::Error::other);
        }
        let _ = pinned;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the Hitch GNOME Shell extension is not available",
        ))
    }

    /// The socket this engine listens on.
    pub fn socket_path(&self) -> &Path {
        &self.inner.socket_path
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct LoopState {
    resolver: Resolver,
    current: Arc<Mutex<ActiveContexts>>,
    reports: Arc<Mutex<HashMap<Source, ContextReport>>>,
    debounce: Duration,
    max_delay: Duration,
}

impl LoopState {
    fn run(mut self, rx: mpsc::Receiver<Input>, mut on_event: impl FnMut(Event)) {
        let mut last: Option<ActiveContexts> = None;
        // (first change, deadline) while a change is waiting.
        let mut pending: Option<(Instant, Instant)> = None;
        let mut zed_hinted = false;
        loop {
            let input = match pending {
                Some((_, deadline)) => {
                    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                        Ok(i) => Some(i),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                None => match rx.recv() {
                    Ok(i) => Some(i),
                    Err(_) => return,
                },
            };
            let Some(input) = input else {
                pending = None;
                let out = self.resolver.resolve();
                if self.resolver.take_zed_title_hint() && !zed_hinted {
                    zed_hinted = true;
                    on_event(Event::ZedNeedsTitleSetting);
                }
                if last.as_ref() != Some(&out) {
                    *lock(&self.current) = out.clone();
                    last = Some(out.clone());
                    on_event(Event::Contexts(out));
                }
                continue;
            };
            if matches!(input, Input::Stop) {
                return;
            }
            if self.apply(input, &mut on_event) {
                let now = Instant::now();
                let first = pending.map_or(now, |(f, _)| f);
                pending = Some((first, (now + self.debounce).min(first + self.max_delay)));
            }
        }
    }

    /// Returns true if the contexts may have changed.
    fn apply(&mut self, input: Input, on_event: &mut impl FnMut(Event)) -> bool {
        let now = Instant::now();
        match input {
            Input::Message(ClientMessage::Context(r)) => {
                lock(&self.reports).insert(r.source.clone(), r.clone());
                self.resolver.report(r, now);
                true
            }
            Input::Message(ClientMessage::Blur { source }) => {
                self.resolver.blur(&source);
                true
            }
            Input::Message(ClientMessage::Command { command }) => {
                on_event(Event::Command(command));
                false
            }
            Input::Message(ClientMessage::Hello { .. } | ClientMessage::Unknown) => false,
            Input::Window(w) => self.resolver.window_focus(w, now),
            Input::WindowGone => {
                self.resolver.clear_window();
                true
            }
            Input::Shell(available) => {
                on_event(Event::ShellExtension { available });
                false
            }
            Input::SetIgnore(ignore) => {
                self.resolver.set_ignore(ignore);
                true
            }
            Input::Stop => false,
        }
    }
}
