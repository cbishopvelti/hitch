//! What the panel needs from the store and the context engine.
//!
//! The panel codes against these two small adapters. They mirror the APIs
//! the `hitch-store` and `hitch-context` crates are growing:
//!
//! - store: `notes_for_all`, `notes_path`, `save_note`, `delete_note`,
//!   returning one [`NoteGroup`] per notes file;
//! - engine: `Event::Contexts` and `Event::Command` from a callback, `current`,
//!   and `set_keep_above` for the pin on Wayland.
//!
//! The engine is real. The store is still a stub until `hitch-store`
//! lands: notes live in memory for the session. Swapping it in should only
//! touch this file.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use hitch_context::ActiveContexts;
use hitch_protocol::Context;
use hitch_store::{NOTES_FILE_NAME, Note};
use iced::Subscription;
use iced::futures::channel::mpsc;

/// The notes of one notes file, shown as one section of the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteGroup {
    /// The HITCH.md (or fallback file) the notes live in.
    pub path: PathBuf,
    /// The context this file was found for.
    pub context: Context,
    /// Most specific first: notes tagged with the current file, then the
    /// rest of the file.
    pub notes: Vec<Note>,
}

/// Stub of `hitch_store::Store`. Keeps notes in memory.
#[derive(Debug, Default)]
pub struct Store {
    files: HashMap<PathBuf, Vec<Note>>,
    next_id: u64,
}

impl Store {
    pub fn for_user() -> Self {
        Self::default()
    }

    /// Groups for the active contexts, most specific first. A notes file is
    /// shown once, under the most specific context that found it.
    pub fn notes_for_all(&self, contexts: &[Context]) -> io::Result<Vec<NoteGroup>> {
        let mut groups: Vec<NoteGroup> = Vec::new();
        for context in contexts {
            let path = self.notes_path(context);
            if groups.iter().any(|g| g.path == path) {
                continue;
            }
            let notes = self.files.get(&path).cloned().unwrap_or_default();
            groups.push(NoteGroup {
                path,
                context: context.clone(),
                notes,
            });
        }
        Ok(groups)
    }

    /// Where a new note for `context` is written.
    pub fn notes_path(&self, context: &Context) -> PathBuf {
        let fallback = hitch_protocol::paths::hitch_dir().unwrap_or_default();
        match context {
            Context::File { project, .. } => project.join(NOTES_FILE_NAME),
            Context::Folder { path } => path.join(NOTES_FILE_NAME),
            Context::Url { url } => fallback
                .join("sites")
                .join(format!("{}.md", super::label::host(url))),
            Context::App { app_id } => fallback.join("apps").join(format!("{app_id}.md")),
        }
    }

    /// Inserts or updates `note` by id, assigning an id when it has none.
    /// Returns the note as stored.
    pub fn save_note(&mut self, path: &Path, mut note: Note) -> io::Result<Note> {
        let notes = self.files.entry(path.to_path_buf()).or_default();
        match note.id.as_deref() {
            Some(id) => match notes.iter_mut().find(|n| n.id.as_deref() == Some(id)) {
                Some(existing) => *existing = note.clone(),
                None => notes.push(note.clone()),
            },
            None => {
                self.next_id += 1;
                note.id = Some(format!("stub{}", self.next_id));
                notes.push(note.clone());
            }
        }
        Ok(note)
    }

    /// Removes the note with `id`. Returns whether it was there.
    pub fn delete_note(&mut self, path: &Path, id: &str) -> io::Result<bool> {
        let Some(notes) = self.files.get_mut(path) else {
            return Ok(false);
        };
        let before = notes.len();
        notes.retain(|n| n.id.as_deref() != Some(id));
        Ok(notes.len() != before)
    }
}

pub use hitch_context::Event;

/// The context engine, plus the clawswap client feeding it, with its events
/// bridged into an iced subscription.
pub struct Engine {
    engine: hitch_context::Engine,
    events: Events,
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
            events: Events(Arc::new(Mutex::new(Some(rx)))),
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

    /// The engine's events, as they happen.
    pub fn subscription(&self) -> Subscription<Event> {
        Subscription::run_with(self.events.clone(), |events| {
            // The receiver is taken once; iced keeps this subscription
            // running for as long as the panel asks for it.
            lock(&events.0)
                .take()
                .unwrap_or_else(|| mpsc::unbounded().1)
        })
    }
}

/// The receiving end of the engine's callback, handed to iced once.
#[derive(Clone)]
struct Events(Arc<Mutex<Option<mpsc::UnboundedReceiver<Event>>>>);

impl Hash for Events {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_store_upserts_and_dedupes() {
        let mut store = Store::for_user();
        let file = Context::File {
            project: "/p".into(),
            file: "src/a.rs".into(),
        };
        let folder = Context::Folder { path: "/p".into() };
        let path = store.notes_path(&folder);
        assert_eq!(path, PathBuf::from("/p/HITCH.md"));

        let saved = store
            .save_note(
                &path,
                Note {
                    title: "Dev setup".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let id = saved.id.clone().unwrap();
        store
            .save_note(
                &path,
                Note {
                    body: "Run it".into(),
                    ..saved
                },
            )
            .unwrap();

        // The File and Folder contexts share one HITCH.md: one group.
        let groups = store.notes_for_all(&[file.clone(), folder]).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].context, file);
        assert_eq!(groups[0].notes.len(), 1);
        assert_eq!(groups[0].notes[0].body, "Run it");

        assert!(store.delete_note(&path, &id).unwrap());
        assert!(!store.delete_note(&path, &id).unwrap());
    }
}
