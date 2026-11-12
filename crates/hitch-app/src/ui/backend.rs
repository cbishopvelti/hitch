//! The panel's links to the context engine and the notes store.
//!
//! Both report from their own threads through callbacks. Each callback
//! feeds a channel that an iced subscription drains, so the panel sees
//! engine events and notes-file changes as ordinary messages.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use hitch_context::ActiveContexts;
use hitch_store::{NotesWatcher, StoreConfig};
use iced::Subscription;
use iced::futures::channel::mpsc;

pub use hitch_store::{NoteGroup, Store};

pub use hitch_context::Event;

const WRITE_CHARS: &str = "write_chars";

/// The context engine, plus the clawswap client feeding it, with its events
/// bridged into an iced subscription.
pub struct Engine {
    engine: hitch_context::Engine,
    events: Events<Event>,
    _clawswap: hitch_context::clawswap::ClawswapHandle,
}

impl Engine {
    /// Starts the engine and its socket. Fails with `AddrInUse` when another
    /// Hitch is already running.
    pub fn start() -> io::Result<Self> {
        let (tx, rx) = mpsc::unbounded();
        let engine =
            hitch_context::Engine::start(hitch_context::Config::default(), move |event| {
                let _ = tx.unbounded_send(event);
            })?;
        let sink = engine.sink();
        let clawswap = hitch_context::clawswap::spawn(
            hitch_context::clawswap::ClawswapConfig::from_env(),
            move |message| sink.message(message),
        );
        Ok(Self {
            engine,
            events: Events::new(rx),
            _clawswap: clawswap,
        })
    }

    /// The contexts that are current right now.
    pub fn current(&self) -> ActiveContexts {
        self.engine.current()
    }

    /// Asks the GNOME Shell extension to keep Hitch's windows above others
    /// on every workspace (Wayland). The pin lasts while the engine's D-Bus
    /// connection does.
    pub fn set_keep_above(&self, pinned: bool) -> io::Result<()> {
        self.engine.set_keep_above(pinned)
    }

    /// The Zellij session to send code blocks to: the one that reported
    /// last, so its last focused pane is the one the user just left.
    pub fn zellij_target(&self) -> Option<super::zellij::Target> {
        let session = self
            .engine
            .last_report(&hitch_protocol::Source::Zellij)?
            .session?;
        Some(super::zellij::Target {
            has_plugin: self.engine.session_has_capability(&session, WRITE_CHARS),
            session,
        })
    }

    /// Hands `write_chars` to a Zellij plugin connected to the socket right
    /// now, if there is one. Returns false if there isn't.
    pub fn deliver_write_chars(&self, text: &str, submit: bool) -> bool {
        self.engine.deliver(
            WRITE_CHARS,
            &hitch_protocol::ServerMessage::WriteChars {
                text: text.to_string(),
                submit,
            },
        )
    }

    /// The engine's events, as they happen.
    pub fn subscription(&self) -> Subscription<Event> {
        self.events.subscription()
    }
}

/// Watches the notes files the panel shows, so edits made in an editor or
/// by `git pull` appear while the panel is open.
pub struct Watcher {
    watcher: Option<NotesWatcher>,
    watched: HashSet<PathBuf>,
    changes: Events<PathBuf>,
}

impl Watcher {
    /// A watcher for notes files named as in `config`. If the OS watcher
    /// can't start, the panel still works and re-reads notes when shown.
    pub fn new(config: &StoreConfig) -> Self {
        let (tx, rx) = mpsc::unbounded();
        let watcher = NotesWatcher::new(config, move |path| {
            let _ = tx.unbounded_send(path);
        })
        .ok();
        Self {
            watcher,
            watched: HashSet::new(),
            changes: Events::new(rx),
        }
    }

    /// Watches exactly these notes files, dropping the ones no longer shown.
    /// A file whose folder doesn't exist is skipped.
    pub fn watch_only(&mut self, files: impl IntoIterator<Item = PathBuf>) {
        let Some(watcher) = &mut self.watcher else {
            return;
        };
        let wanted: HashSet<PathBuf> = files.into_iter().collect();
        for gone in self.watched.difference(&wanted) {
            let _ = watcher.unwatch_file(gone);
        }
        self.watched.retain(|f| wanted.contains(f));
        for file in wanted {
            if !self.watched.contains(&file) && watcher.watch_file(&file).is_ok() {
                self.watched.insert(file);
            }
        }
    }

    /// The path of each watched notes file that changed. Not debounced.
    pub fn subscription(&self) -> Subscription<PathBuf> {
        self.changes.subscription()
    }
}

/// The receiving end of a callback's channel, handed to iced once.
struct Events<T>(Arc<Mutex<Option<mpsc::UnboundedReceiver<T>>>>);

impl<T> Clone for Events<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Hash for Events<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}

impl<T: Send + 'static> Events<T> {
    fn new(rx: mpsc::UnboundedReceiver<T>) -> Self {
        Self(Arc::new(Mutex::new(Some(rx))))
    }

    fn subscription(&self) -> Subscription<T> {
        Subscription::run_with(self.clone(), |events| {
            // The receiver is taken once; iced keeps the subscription
            // running for as long as the panel asks for it.
            lock(&events.0)
                .take()
                .unwrap_or_else(|| mpsc::unbounded().1)
        })
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
