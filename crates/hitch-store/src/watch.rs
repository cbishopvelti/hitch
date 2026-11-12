//! Watching notes files for edits made outside Hitch: in an editor, or by
//! a `git pull` or checkout.
//!
//! Editors and git usually replace a file rather than write into it, which
//! a watch on the file itself would miss, so the watcher watches each
//! notes file's folder (not recursively) and filters events down to the
//! notes files. The callback runs on the watcher's own thread, gets the
//! notes file's path, and should hand off quickly; it is not debounced, so
//! one save can call it more than once.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::project::normalize;
use crate::{Store, StoreConfig, StoreError};

#[derive(Debug, Default)]
struct Filter {
    file_name: String,
    /// Folders whose notes file is watched.
    folders: HashSet<PathBuf>,
    /// Fallback folders, where every `.md` file is a notes file.
    fallback: HashSet<PathBuf>,
}

impl Filter {
    fn wants(&self, path: &Path) -> bool {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return false;
        };
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            return false; // our own temporary files, editor swap files
        }
        (name == self.file_name && self.folders.contains(parent))
            || (self.fallback.contains(parent) && name.ends_with(".md"))
    }
}

/// Watches notes files and calls back with the path of each one that
/// changed, appeared or was removed.
pub struct NotesWatcher {
    watcher: RecommendedWatcher,
    filter: Arc<Mutex<Filter>>,
}

impl std::fmt::Debug for NotesWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotesWatcher").finish_non_exhaustive()
    }
}

impl NotesWatcher {
    /// A watcher for notes files named as in `config`. Nothing is watched
    /// until [`NotesWatcher::watch_file`] or [`NotesWatcher::watch_store`].
    pub fn new(
        config: &StoreConfig,
        on_change: impl Fn(PathBuf) + Send + 'static,
    ) -> Result<Self, StoreError> {
        let filter = Arc::new(Mutex::new(Filter {
            file_name: config.file_name.clone(),
            ..Filter::default()
        }));
        let events = Arc::clone(&filter);
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            let wanted: Vec<PathBuf> = {
                let filter = events.lock().unwrap_or_else(|e| e.into_inner());
                event
                    .paths
                    .into_iter()
                    .filter(|p| filter.wants(p))
                    .collect()
            };
            for path in wanted {
                on_change(path);
            }
        })?;
        Ok(Self { watcher, filter })
    }

    /// Watch one notes file (it need not exist yet, but its folder must).
    /// `false` if it was already watched.
    pub fn watch_file(&mut self, notes_file: &Path) -> Result<bool, StoreError> {
        let notes_file = normalize(notes_file);
        let Some(folder) = notes_file.parent().map(Path::to_path_buf) else {
            return Ok(false);
        };
        if self.lock().folders.contains(&folder) {
            return Ok(false);
        }
        self.watcher.watch(&folder, RecursiveMode::NonRecursive)?;
        self.lock().folders.insert(folder);
        Ok(true)
    }

    /// Stop watching a notes file.
    pub fn unwatch_file(&mut self, notes_file: &Path) -> Result<(), StoreError> {
        let notes_file = normalize(notes_file);
        if let Some(folder) = notes_file.parent()
            && self.lock().folders.remove(folder)
        {
            self.watcher.unwatch(folder)?;
        }
        Ok(())
    }

    /// Watch every known location and the fallback folders (created if
    /// missing, so new fallback files are seen too). Locations whose folder
    /// is gone are skipped.
    pub fn watch_store(&mut self, store: &Store) -> Result<(), StoreError> {
        let config = store.config();
        for dir in [config.apps_dir(), config.sites_dir()] {
            std::fs::create_dir_all(&dir).map_err(|e| StoreError::io(&dir, e))?;
            if self.lock().fallback.insert(dir.clone()) {
                self.watcher.watch(&dir, RecursiveMode::NonRecursive)?;
            }
        }
        for file in store.known_files() {
            if file
                .parent()
                .is_some_and(|d| self.lock().fallback.contains(d))
            {
                continue;
            }
            match self.watch_file(&file) {
                Ok(_) => {}
                Err(StoreError::Watch(e)) if is_missing(&e) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Filter> {
        self.filter.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn is_missing(e: &notify::Error) -> bool {
    match &e.kind {
        notify::ErrorKind::PathNotFound => true,
        notify::ErrorKind::Io(io) => io.kind() == std::io::ErrorKind::NotFound,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Note};
    use std::sync::mpsc;
    use std::time::Duration;

    fn next(rx: &mpsc::Receiver<PathBuf>) -> Option<PathBuf> {
        rx.recv_timeout(Duration::from_secs(5)).ok()
    }

    #[test]
    fn sees_outside_edits_to_notes_files_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let store = Store::new(StoreConfig::new(root.join(".hitch")));
        let project = root.join("boodle");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        let notes = project.join("HITCH.md");
        store.save_note(&notes, Note::new("A", "a")).unwrap();

        let (tx, rx) = mpsc::channel();
        let mut watcher = NotesWatcher::new(store.config(), move |p| {
            let _ = tx.send(p);
        })
        .unwrap();
        watcher.watch_store(&store).unwrap();

        // Other files in the project folder are ignored.
        std::fs::write(project.join("README.md"), "hi").unwrap();
        // A git-style replace of the notes file is seen.
        let tmp_file = project.join("HITCH.md.new");
        std::fs::write(&tmp_file, "## B\nb\n").unwrap();
        std::fs::rename(&tmp_file, &notes).unwrap();
        assert_eq!(next(&rx), Some(notes.clone()));
        while rx.recv_timeout(Duration::from_millis(200)).is_ok() {}

        // A new fallback file is seen too.
        let app = store.notes_path(&Context::App {
            app_id: "figma".into(),
        });
        store.save_note(&app, Note::new("C", "c")).unwrap();
        assert_eq!(next(&rx), Some(app));
        while rx.recv_timeout(Duration::from_millis(200)).is_ok() {}

        watcher.unwatch_file(&notes).unwrap();
        std::fs::write(&notes, "## D\n").unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_millis(500)).ok(), None);
    }
}
