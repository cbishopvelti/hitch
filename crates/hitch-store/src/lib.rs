//! Note storage for Hitch.
//!
//! Notes live next to the work: each project folder keeps them in a
//! `HITCH.md` file at its root (see `docs/SPEC.md`, "Data model and
//! storage"). This crate owns:
//!
//! - parsing and writing HITCH.md (one H2 section per note, optional
//!   `<!-- hitch ... -->` metadata comment),
//! - finding the project folder for a path (nearest HITCH.md, else nearest
//!   `.git`, else the folder itself),
//! - the fallback files in `~/.hitch/` for app and URL contexts,
//! - the known-locations list and the SQLite full-text index.
//!
//! The HITCH.md files are the source of truth; the index is a disposable
//! cache rebuilt from them.

use std::path::PathBuf;

pub use hitch_protocol::Context;

/// Default notes file name. A user setting can change it.
pub const NOTES_FILE_NAME: &str = "HITCH.md";

/// One note: an H2 section of a HITCH.md file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Note {
    /// Stable id from the metadata comment. `None` for a section written by
    /// hand with no comment.
    pub id: Option<String>,
    /// The H2 heading text.
    pub title: String,
    /// Markdown body below the heading and metadata comment.
    pub body: String,
    /// File the note is tagged to, relative to the project folder. `None`
    /// means project-wide.
    pub file: Option<PathBuf>,
    /// Creation date as written in the metadata comment (YYYY-MM-DD).
    pub created: Option<String>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_file_name() {
        assert_eq!(super::NOTES_FILE_NAME, "HITCH.md");
    }
}
