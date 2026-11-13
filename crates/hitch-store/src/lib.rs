//! Note storage for Hitch.
//!
//! Notes live next to the work: each project folder keeps them in a
//! `HITCH.md` file at its root (see `docs/SPEC.md`, "Data model and
//! storage"). This crate owns:
//!
//! - parsing and writing HITCH.md ([`format`]): one H2 section per note,
//!   with an optional `<!-- hitch ... -->` metadata comment, written back
//!   so that only changed notes show up in a diff,
//! - finding the project folder for a path ([`find_project`]): the nearest
//!   folder with a HITCH.md or a `.git`,
//! - reading and saving notes for a context ([`Store`]), including the
//!   fallback files in `~/.hitch/` for app and URL contexts,
//! - the known-locations list ([`locations`]), the SQLite full-text index
//!   ([`index`]) and the file watcher ([`watch`]).
//!
//! The HITCH.md files are the source of truth; the index is a disposable
//! cache rebuilt from them.

use std::path::{Path, PathBuf};

pub use hitch_protocol::Context;

pub mod format;
mod id;
pub mod index;
pub mod locations;
mod project;
mod store;
pub mod watch;

pub use format::NotesFile;
pub use index::{Index, SearchHit};
pub use project::{
    SaveFolder, ancestor_notes_folders, app_file_name, find_project, git_root, normalize,
    outside_repo, relative_to, save_folders, site_file_name, site_key,
};
pub use store::{NoteGroup, Store, StoreConfig};
pub use watch::NotesWatcher;

/// Default notes file name. A user setting can change it.
pub const NOTES_FILE_NAME: &str = "HITCH.md";

/// One note: an H2 section of a HITCH.md file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Note {
    /// Stable id from the metadata comment. A note read from a file always
    /// has one: a hand-written section with no comment gets an id derived
    /// from its heading. `None` only for a new note not yet saved.
    pub id: Option<String>,
    /// The H2 heading text.
    pub title: String,
    /// Markdown body below the heading and metadata comment, with no
    /// leading or trailing blank lines.
    pub body: String,
    /// Files the note is hitched to, relative to the project folder, using
    /// `/` separators. A folder hitches the note to every file inside it.
    /// Empty means project-wide.
    pub files: Vec<PathBuf>,
    /// Creation date as written in the metadata comment (YYYY-MM-DD).
    pub created: Option<String>,
    /// Metadata keys Hitch doesn't know, kept in order so they survive an
    /// edit by an older or newer version.
    pub extra: Vec<(String, String)>,
}

impl Note {
    /// A new, project-wide note.
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            ..Self::default()
        }
    }

    /// A new note from typed text: the first line (without any leading
    /// `#`s) is the title, the rest the body.
    pub fn from_text(text: &str) -> Self {
        let text = text.trim();
        let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
        Self::new(first.trim_start_matches('#').trim(), rest.trim())
    }

    /// True when the note isn't hitched to any file.
    pub fn is_project_wide(&self) -> bool {
        self.files.is_empty()
    }

    /// True when the note is hitched to `file` (relative to the project),
    /// directly or through a folder that contains it.
    pub fn is_hitched_to(&self, file: &Path) -> bool {
        let file = format::path_tag(file);
        self.files.iter().any(|tag| {
            let tag = format::path_tag(tag);
            let tag = tag.trim_end_matches('/');
            !tag.is_empty()
                && (file == tag
                    || file
                        .strip_prefix(tag)
                        .is_some_and(|rest| rest.starts_with('/')))
        })
    }
}

/// Errors from reading or writing notes.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{}: not valid UTF-8, so Hitch won't rewrite it", .0.display())]
    NotUtf8(PathBuf),
    #[error("no note with id {id} in {}", path.display())]
    NoteNotFound { path: PathBuf, id: String },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("search index: {0}")]
    Index(#[from] rusqlite::Error),
    #[error("file watcher: {0}")]
    Watch(#[from] notify::Error),
}

impl StoreError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// Write a file by writing a temporary file next to it and renaming it into
/// place, so a crash never leaves half a file. Keeps the old file's
/// permissions, and writes through a symlink rather than replacing it.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> Result<(), StoreError> {
    use std::io::Write;

    let target = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            std::fs::canonicalize(path).map_err(|e| StoreError::io(path, e))?
        }
        _ => path.to_path_buf(),
    };
    let dir = target.parent().unwrap_or(Path::new("."));
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        id::generate()
    ));

    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(contents.as_bytes())?;
        if let Ok(meta) = std::fs::metadata(&target) {
            file.set_permissions(meta.permissions())?;
        }
        file.sync_all()?;
        std::fs::rename(&tmp, &target)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(StoreError::io(&target, e));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_file_name() {
        assert_eq!(super::NOTES_FILE_NAME, "HITCH.md");
    }

    #[test]
    fn from_text() {
        let n = Note::from_text("  # Deploy\n\nrun `make ship`\n");
        assert_eq!(n.title, "Deploy");
        assert_eq!(n.body, "run `make ship`");
        assert_eq!(Note::from_text("just a title").body, "");
    }

    #[test]
    fn hitched_to_files_and_folders() {
        let n = Note {
            files: vec!["src/parser.rs".into(), "docs/".into()],
            ..Note::default()
        };
        assert!(n.is_hitched_to(Path::new("src/parser.rs")));
        assert!(n.is_hitched_to(Path::new("docs/a/b.md")));
        assert!(!n.is_hitched_to(Path::new("src/parser.rs.bak")));
        assert!(!n.is_hitched_to(Path::new("docsx/a.md")));
        assert!(!Note::default().is_hitched_to(Path::new("a")));
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_keeps_mode_and_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real.md");
        std::fs::write(&real, "old").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = tmp.path().join("HITCH.md");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        write_atomic(&link, "new").unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
        let mode = std::fs::metadata(&real).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let leftovers = std::fs::read_dir(tmp.path()).unwrap().count();
        assert_eq!(leftovers, 2);
    }
}
