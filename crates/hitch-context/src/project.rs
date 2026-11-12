//! Finding the project folder for a path (spec, matching rules 1 and 2).

use std::path::{Path, PathBuf};

/// Maps a path to the folder whose notes file holds its notes.
///
/// The engine calls this for every file or working folder a source reports.
/// [`FsProjectFinder`] is the default; the app can swap in another one (for
/// example one backed by the store's known locations).
pub trait ProjectFinder: Send {
    /// The project folder for `dir`, a folder on disk. `hint` is the project
    /// root the source reported, if any (a Zed worktree).
    fn project_for(&self, dir: &Path, hint: Option<&Path>) -> PathBuf;

    /// Folders above `project` that have their own notes file, nearest first.
    /// Their notes are shown too, after the project's.
    fn parents_with_notes(&self, project: &Path) -> Vec<PathBuf>;
}

/// Looks at the file system.
///
/// Walking up from the path, the first folder that has the notes file or a
/// `.git` entry is the project. That way `~/playground/HITCH.md` doesn't
/// swallow a repo inside `~/playground` that has no notes yet; its notes
/// still show as a parent. If nothing is found, the reported project root is
/// used when the path is inside it, else the folder itself.
#[derive(Debug, Clone)]
pub struct FsProjectFinder {
    pub notes_file_name: String,
}

impl Default for FsProjectFinder {
    fn default() -> Self {
        Self {
            notes_file_name: hitch_store::NOTES_FILE_NAME.to_owned(),
        }
    }
}

impl FsProjectFinder {
    fn has_notes(&self, dir: &Path) -> bool {
        dir.join(&self.notes_file_name).is_file()
    }
}

impl ProjectFinder for FsProjectFinder {
    fn project_for(&self, dir: &Path, hint: Option<&Path>) -> PathBuf {
        for d in dir.ancestors() {
            if self.has_notes(d) || d.join(".git").exists() {
                return d.to_path_buf();
            }
        }
        match hint {
            Some(h) if dir.starts_with(h) => h.to_path_buf(),
            _ => dir.to_path_buf(),
        }
    }

    fn parents_with_notes(&self, project: &Path) -> Vec<PathBuf> {
        project
            .ancestors()
            .skip(1)
            .filter(|d| self.has_notes(d))
            .map(Path::to_path_buf)
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::fs;

    /// A fresh, empty temporary folder, removed on drop.
    pub(crate) struct TempDir(pub PathBuf);

    impl TempDir {
        pub(crate) fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "hitch-ctx-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }

        pub(crate) fn path(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }

        pub(crate) fn file(&self, rel: &str) -> PathBuf {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, "").unwrap();
            p
        }

        pub(crate) fn dir(&self, rel: &str) -> PathBuf {
            let p = self.0.join(rel);
            fs::create_dir_all(&p).unwrap();
            p
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn nearest_notes_or_git_wins() {
        let t = TempDir::new("finder");
        t.file("play/HITCH.md");
        t.dir("play/boodle/.git");
        let src = t.dir("play/boodle/src");
        t.file("play/other/HITCH.md");
        let deep = t.dir("play/other/a/b");
        let f = FsProjectFinder::default();

        assert_eq!(f.project_for(&src, None), t.path("play/boodle"));
        assert_eq!(f.project_for(&deep, None), t.path("play/other"));
        assert_eq!(
            f.parents_with_notes(&t.path("play/boodle")),
            vec![t.path("play")]
        );
    }

    #[test]
    fn notes_file_beats_git_root_when_nearer() {
        let t = TempDir::new("finder-sub");
        t.dir("repo/.git");
        t.file("repo/sub/HITCH.md");
        let d = t.dir("repo/sub/x");
        let f = FsProjectFinder::default();
        assert_eq!(f.project_for(&d, None), t.path("repo/sub"));
    }

    #[test]
    fn falls_back_to_hint_then_folder() {
        // Run under a folder with nothing above it that tests control, so
        // only check paths that can't have a .git or HITCH.md: use a custom
        // notes name and rely on temp having no .git.
        let t = TempDir::new("finder-none");
        let d = t.dir("w/src");
        let f = FsProjectFinder {
            notes_file_name: "NO-SUCH-NOTES-FILE.md".into(),
        };
        if f.project_for(&d, None) == d {
            assert_eq!(f.project_for(&d, Some(&t.path("w"))), t.path("w"));
            assert_eq!(f.project_for(&d, Some(Path::new("/elsewhere"))), d);
        }
    }
}
