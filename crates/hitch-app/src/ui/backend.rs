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
//! Until those land, both are stubs: notes live in memory for the session
//! and the only context is the folder Hitch was started in. Swapping in the
//! real crates should only touch this file.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use hitch_context::ActiveContexts;
use hitch_protocol::{Command, Context};
use hitch_store::{NOTES_FILE_NAME, Note};
use iced::Subscription;

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

/// What the context engine tells the panel.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Only the real engine sends these; the stub never does.
pub enum Event {
    /// The active contexts changed (debounced, most specific first).
    Contexts(ActiveContexts),
    /// A command from `hitch toggle` and friends, or a global shortcut.
    Command(Command),
}

/// Stub of `hitch_context::Engine`.
#[derive(Debug, Default)]
pub struct Engine {
    start_dir: Option<PathBuf>,
}

impl Engine {
    pub fn start() -> Self {
        Self {
            start_dir: std::env::current_dir().ok(),
        }
    }

    /// The contexts that are current right now.
    pub fn current(&self) -> ActiveContexts {
        ActiveContexts::new(
            self.start_dir
                .iter()
                .map(|path| Context::Folder { path: path.clone() })
                .collect(),
        )
    }

    /// Asks the GNOME Shell extension to keep Hitch's windows above others
    /// on every workspace (Wayland). The real engine holds the D-Bus
    /// connection the pin is tied to.
    pub fn set_keep_above(&self, _pinned: bool) {}

    /// Context changes and commands, as they happen. The real engine calls
    /// back from its own threads; this forwards them into iced through a
    /// channel.
    pub fn subscription(&self) -> Subscription<Event> {
        Subscription::none()
    }
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
