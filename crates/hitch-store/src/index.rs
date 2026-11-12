//! The search index: an SQLite cache with an FTS5 full-text table over
//! every known notes file.
//!
//! The notes files are the source of truth. The index records each file's
//! modification time and size, so [`Index::sync`] only re-reads files that
//! changed, and the whole database can be deleted at any time: it is
//! rebuilt on the next sync.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::{Connection, OptionalExtension, params};

use crate::format::{NotesFile, path_tag};
use crate::{Note, Store, StoreError};

/// Bumped whenever the schema changes; an index with another version is
/// dropped and rebuilt.
const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE files (
    path TEXT PRIMARY KEY,
    mtime_ns INTEGER NOT NULL,
    size INTEGER NOT NULL
);
CREATE VIRTUAL TABLE notes USING fts5(
    title,
    body,
    path UNINDEXED,
    note_id UNINDEXED,
    files UNINDEXED,
    created UNINDEXED,
    ord UNINDEXED,
    tokenize = 'unicode61 remove_diacritics 2'
);
";

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    /// The notes file the note is in. Its folder is the context to jump to.
    pub path: PathBuf,
    pub note: Note,
    /// A short extract of the body around the match, for the result list.
    pub snippet: String,
}

/// What a sync did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Files read again because they were new or had changed.
    pub indexed: usize,
    /// Files dropped from the index because they are gone.
    pub removed: usize,
}

/// The search index.
#[derive(Debug)]
pub struct Index {
    conn: Connection,
}

impl Index {
    /// Open (or create) the index at `path`. An index that can't be read,
    /// or has an older schema, is thrown away and started again.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| StoreError::io(dir, e))?;
        }
        match Self::from_conn(Connection::open(path)?) {
            Ok(index) => Ok(index),
            Err(_) => {
                for suffix in ["", "-wal", "-shm"] {
                    let mut p = path.as_os_str().to_owned();
                    p.push(suffix);
                    let _ = std::fs::remove_file(PathBuf::from(p));
                }
                Self::from_conn(Connection::open(path)?)
            }
        }
    }

    /// An index that lives only in memory.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> Result<Self, StoreError> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != SCHEMA_VERSION {
            conn.execute_batch("DROP TABLE IF EXISTS notes; DROP TABLE IF EXISTS files;")?;
            conn.execute_batch(SCHEMA)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        Ok(Self { conn })
    }

    /// Bring the index in line with the store's known notes files.
    pub fn sync_store(&mut self, store: &Store) -> Result<SyncReport, StoreError> {
        self.sync(&store.known_files())
    }

    /// Bring the index in line with `files`: re-read the ones that are new
    /// or changed, and drop any indexed file not in the list.
    pub fn sync(&mut self, files: &[PathBuf]) -> Result<SyncReport, StoreError> {
        let mut report = SyncReport::default();
        let tx = self.conn.transaction()?;
        for file in files {
            let Some((mtime, size)) = stamp(file) else {
                continue;
            };
            let key = key(file);
            let known: Option<(i64, i64)> = tx
                .query_row(
                    "SELECT mtime_ns, size FROM files WHERE path = ?1",
                    [&key],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if known != Some((mtime, size)) {
                index_file(&tx, file, &key, mtime, size)?;
                report.indexed += 1;
            }
        }
        let wanted: std::collections::HashSet<String> = files.iter().map(|f| key(f)).collect();
        let indexed: Vec<String> = {
            let mut stmt = tx.prepare("SELECT path FROM files")?;
            stmt.query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?
        };
        for path in indexed {
            if !wanted.contains(&path) || !Path::new(&path).is_file() {
                remove(&tx, &path)?;
                report.removed += 1;
            }
        }
        tx.commit()?;
        Ok(report)
    }

    /// Re-read one file now, such as after the watcher saw it change. A
    /// file that is gone is dropped from the index.
    pub fn refresh(&mut self, file: &Path) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        let key = key(file);
        match stamp(file) {
            Some((mtime, size)) => index_file(&tx, file, &key, mtime, size)?,
            None => remove(&tx, &key)?,
        }
        tx.commit()?;
        Ok(())
    }

    /// Full-text search over titles and bodies, best match first. Each word
    /// matches as a prefix, and all words must match.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, StoreError> {
        let Some(fts) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT path, note_id, title, body, files, created,
                    snippet(notes, 1, '', '', '…', 12)
             FROM notes WHERE notes MATCH ?1
             ORDER BY bm25(notes, 10.0, 1.0), path, ord
             LIMIT ?2",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let hits = stmt
            .query_map(params![fts, limit], |r| {
                let files: String = r.get(4)?;
                Ok(SearchHit {
                    path: PathBuf::from(r.get::<_, String>(0)?),
                    note: Note {
                        id: Some(r.get(1)?),
                        title: r.get(2)?,
                        body: r.get(3)?,
                        files: files
                            .split('\n')
                            .filter(|f| !f.is_empty())
                            .map(PathBuf::from)
                            .collect(),
                        created: r.get(5)?,
                        extra: Vec::new(),
                    },
                    snippet: r.get(6)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(hits)
    }

    /// Number of notes in the index.
    pub fn len(&self) -> Result<usize, StoreError> {
        let n: i64 = self
            .conn
            .query_row("SELECT count(*) FROM notes", [], |r| r.get(0))?;
        Ok(usize::try_from(n).unwrap_or(0))
    }

    /// True when the index holds no notes.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
    }
}

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn stamp(path: &Path) -> Option<(i64, i64)> {
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    Some((mtime, i64::try_from(meta.len()).unwrap_or(i64::MAX)))
}

fn remove(conn: &Connection, key: &str) -> Result<(), StoreError> {
    conn.execute("DELETE FROM notes WHERE path = ?1", [key])?;
    conn.execute("DELETE FROM files WHERE path = ?1", [key])?;
    Ok(())
}

fn index_file(
    conn: &Connection,
    file: &Path,
    key: &str,
    mtime: i64,
    size: i64,
) -> Result<(), StoreError> {
    conn.execute("DELETE FROM notes WHERE path = ?1", [key])?;
    // A file that can't be read as text is indexed as empty; the stamp
    // still goes in so it isn't re-read until it changes.
    let text = std::fs::read(file)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    let parsed = NotesFile::parse(&text);
    let mut insert = conn.prepare_cached(
        "INSERT INTO notes (title, body, path, note_id, files, created, ord)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for (ord, note) in parsed.notes().enumerate() {
        let files: Vec<String> = note.files.iter().map(|f| path_tag(f)).collect();
        insert.execute(params![
            note.title,
            note.body,
            key,
            note.id,
            files.join("\n"),
            note.created,
            i64::try_from(ord).unwrap_or(i64::MAX),
        ])?;
    }
    conn.execute(
        "INSERT INTO files (path, mtime_ns, size) VALUES (?1, ?2, ?3)
         ON CONFLICT(path) DO UPDATE SET mtime_ns = ?2, size = ?3",
        params![key, mtime, size],
    )?;
    Ok(())
}

/// Turn what the user typed into an FTS5 query: each word becomes a quoted
/// prefix term, so FTS5 syntax characters are taken literally.
fn fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, StoreConfig};
    use std::fs;

    #[test]
    fn query_escaping() {
        assert_eq!(fts_query("  "), None);
        assert_eq!(fts_query("- *"), None);
        assert_eq!(
            fts_query(r#"dep"loy NOT x"#).unwrap(),
            r#""dep""loy"* "NOT"* "x"*"#
        );
    }

    #[test]
    fn sync_search_and_refresh() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let store = Store::new(StoreConfig::new(root.join(".hitch")));
        let project = root.join("boodle");
        fs::create_dir_all(project.join(".git")).unwrap();
        let notes = project.join("HITCH.md");
        store
            .save_note(&notes, Note::new("Deploy", "Run `make ship` from the café"))
            .unwrap();
        store
            .save_note(
                &notes,
                Note {
                    files: vec!["src/parser.rs".into()],
                    ..Note::new("Parser bug", "Fails on empty front matter")
                },
            )
            .unwrap();
        let app = store.notes_path(&Context::App {
            app_id: "figma".into(),
        });
        store
            .save_note(&app, Note::new("Shortcuts", "deploy previews with Ctrl+D"))
            .unwrap();

        let mut index = Index::open(&store.config().index_path()).unwrap();
        assert_eq!(
            index.sync_store(&store).unwrap(),
            SyncReport {
                indexed: 2,
                removed: 0
            }
        );
        assert_eq!(index.len().unwrap(), 3);
        // Nothing changed: nothing re-read.
        assert_eq!(index.sync_store(&store).unwrap(), SyncReport::default());

        let hits = index.search("depl", 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].note.title, "Deploy", "title matches rank first");
        assert_eq!(hits[0].path, notes);
        assert_eq!(hits[1].path, app);

        let hits = index.search("cafe", 10).unwrap();
        assert_eq!(hits.len(), 1, "diacritics are folded");
        let hits = index.search("front parser", 10).unwrap();
        assert_eq!(hits[0].note.files, vec![PathBuf::from("src/parser.rs")]);
        assert!(index.search("\"unbalanced", 10).unwrap().is_empty());

        // An edit made outside Hitch, then a refresh.
        fs::write(&notes, "## Deploy\nnow `just ship`\n").unwrap();
        index.refresh(&notes).unwrap();
        assert!(index.search("parser", 10).unwrap().is_empty());
        assert_eq!(index.search("just", 10).unwrap().len(), 1);

        // A deleted file drops out on the next sync.
        fs::remove_file(&app).unwrap();
        assert_eq!(index.sync_store(&store).unwrap().removed, 1);
        assert!(index.search("previews", 10).unwrap().is_empty());

        // Reopening keeps the index.
        drop(index);
        let index = Index::open(&store.config().index_path()).unwrap();
        assert_eq!(index.len().unwrap(), 1);
    }

    #[test]
    fn a_corrupt_index_is_rebuilt() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("index.sqlite");
        fs::write(
            &path,
            "not a database, just some bytes that are long enough",
        )
        .unwrap();
        let index = Index::open(&path).unwrap();
        assert!(index.is_empty().unwrap());
    }
}
