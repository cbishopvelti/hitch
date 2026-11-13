//! Finding the project folder for a path, and the fallback files for
//! contexts with no folder.

use std::path::{Component, Path, PathBuf};

/// The project folder for `path`: walking up from it (from its folder, if
/// it is a file), the first folder that holds the notes file or a `.git`
/// wins. `None` when no folder on the way up has either.
///
/// "Nearest of either" means a git repo with no HITCH.md yet is still its
/// own project, rather than being swallowed by a HITCH.md further up such
/// as `~/playground/HITCH.md`. Notes in those parent folders are shown as
/// well; see [`ancestor_notes_folders`].
///
/// `.git` may be a folder or a file (worktrees and submodules).
pub fn find_project(path: &Path, file_name: &str) -> Option<PathBuf> {
    let start = start_folder(path);
    start
        .ancestors()
        .find(|dir| dir.join(file_name).is_file() || dir.join(".git").exists())
        .map(Path::to_path_buf)
}

/// Folders strictly above `project` that hold the notes file, nearest
/// first. The panel shows their notes after the project's own.
pub fn ancestor_notes_folders(project: &Path, file_name: &str) -> Vec<PathBuf> {
    project
        .ancestors()
        .skip(1)
        .filter(|dir| dir.join(file_name).is_file())
        .map(Path::to_path_buf)
        .collect()
}

/// The git repo `path` is in: walking up from it (from its folder, if it
/// is a file), the first folder with a `.git` folder or file.
pub fn git_root(path: &Path) -> Option<PathBuf> {
    start_folder(path)
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

/// The root of the git repo `path` is in, when `folder` lies above it, so a
/// notes file saved in `folder` would not be committed with that repo.
/// `None` when `path` isn't in a repo or `folder` is inside it (or beside
/// it: only folders the repo is inside count as "above").
pub fn outside_repo(folder: &Path, path: &Path) -> Option<PathBuf> {
    let root = git_root(path)?;
    let folder = normalize(folder);
    (root != folder && root.starts_with(&folder)).then_some(root)
}

/// A folder a new note can be saved to (spec F14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveFolder {
    pub path: PathBuf,
    /// The folder already has the notes file. Saving to one that doesn't
    /// creates it.
    pub has_notes: bool,
    /// The root of the git repo `path` is in, when this folder is above it.
    /// The panel warns before saving there.
    pub outside_repo: Option<PathBuf>,
}

/// The folders a note about `path` can be saved to, nearest first (spec
/// F14): every folder from `path`'s own folder up to `project` (the
/// default), then each folder further up that already has the notes file.
/// When `path` isn't inside `project`, the list starts at `project`.
pub fn save_folders(path: &Path, project: &Path, file_name: &str) -> Vec<SaveFolder> {
    let project = normalize(project);
    let start = start_folder(path);
    let start = if start.starts_with(&project) {
        start
    } else {
        project.clone()
    };
    let up_to_project = start
        .ancestors()
        .take_while(|dir| dir.starts_with(&project))
        .map(Path::to_path_buf);
    let above: Vec<PathBuf> = ancestor_notes_folders(&project, file_name);
    let root = git_root(path);
    up_to_project
        .chain(above)
        .map(|dir| SaveFolder {
            has_notes: dir.join(file_name).is_file(),
            outside_repo: root
                .clone()
                .filter(|root| *root != dir && root.starts_with(&dir)),
            path: dir,
        })
        .collect()
}

/// The folder to start from: `path` itself if it is a folder, else its
/// parent. A path that doesn't exist is treated as a file.
pub(crate) fn start_folder(path: &Path) -> PathBuf {
    let path = normalize(path);
    if path.is_dir() {
        path
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or(path)
    }
}

/// Make a path absolute and drop `.` and `..` without touching the disk,
/// so a deleted file still resolves.
pub fn normalize(path: &Path) -> PathBuf {
    let joined;
    let path = if path.is_absolute() {
        path
    } else {
        joined = std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf());
        &joined
    };
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// `path` relative to `base`, if it is inside it.
pub fn relative_to(path: &Path, base: &Path) -> Option<PathBuf> {
    normalize(path)
        .strip_prefix(normalize(base))
        .ok()
        .map(Path::to_path_buf)
}

/// File name for an app's fallback notes, from its desktop app id.
pub fn app_file_name(app_id: &str) -> String {
    format!("{}.md", sanitize(app_id))
}

/// File name for a web site's fallback notes: the URL's host, plus its port
/// when there is one, so two dev servers on localhost stay apart.
pub fn site_file_name(url: &str) -> String {
    format!("{}.md", sanitize(&site_key(url)))
}

/// The host (and port) part of a URL, lowercased. Falls back to the whole
/// string for URLs with no `//` authority, such as `about:blank`.
pub fn site_key(url: &str) -> String {
    let rest = match url.find("://") {
        Some(i) => &url[i + 3..],
        None => return url.to_ascii_lowercase(),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or(authority);
    host.to_ascii_lowercase()
}

fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_start_matches('.');
    if trimmed.is_empty() {
        "_".to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn nearest_notes_file_or_git_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let playground = root.join("playground");
        let boodle = playground.join("boodle");
        let src = boodle.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(playground.join("HITCH.md"), "").unwrap();
        fs::write(src.join("main.rs"), "").unwrap();

        // No .git or HITCH.md in boodle yet: the parent's HITCH.md wins.
        assert_eq!(
            find_project(&src.join("main.rs"), "HITCH.md"),
            Some(playground.clone())
        );

        // A git repo is its own project even before it has notes.
        fs::create_dir(boodle.join(".git")).unwrap();
        assert_eq!(
            find_project(&src.join("main.rs"), "HITCH.md"),
            Some(boodle.clone())
        );
        assert_eq!(find_project(&src, "HITCH.md"), Some(boodle.clone()));
        // A file that doesn't exist (yet) still resolves.
        assert_eq!(
            find_project(&src.join("new.rs"), "HITCH.md"),
            Some(boodle.clone())
        );
        assert_eq!(
            ancestor_notes_folders(&boodle, "HITCH.md"),
            vec![playground.clone()]
        );

        // The configured file name is the one that counts.
        assert_eq!(find_project(&playground, "NOTES.md"), None);
    }

    #[test]
    fn save_folders_nearest_first_and_repo_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let playground = root.join("playground");
        let boodle = playground.join("boodle");
        let src = boodle.join("src").join("parse");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir(boodle.join(".git")).unwrap();
        fs::write(playground.join("HITCH.md"), "").unwrap();
        fs::write(src.join("lexer.rs"), "").unwrap();
        let file = src.join("lexer.rs");

        let folders = save_folders(&file, &boodle, "HITCH.md");
        let paths: Vec<_> = folders.iter().map(|f| f.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                src.clone(),
                boodle.join("src"),
                boodle.clone(),
                playground.clone()
            ]
        );
        assert!(folders[..3].iter().all(|f| f.outside_repo.is_none()));
        assert_eq!(folders[3].outside_repo, Some(boodle.clone()));
        assert!(folders[3].has_notes);
        assert!(!folders[2].has_notes);

        assert_eq!(git_root(&file), Some(boodle.clone()));
        assert_eq!(outside_repo(&playground, &file), Some(boodle.clone()));
        assert_eq!(outside_repo(&boodle, &file), None);
        assert_eq!(outside_repo(&src, &file), None);
        // Not in a repo: nothing to warn about.
        assert_eq!(outside_repo(root, &playground.join("HITCH.md")), None);

        // A folder context starts at the folder itself.
        let folders = save_folders(&boodle, &boodle, "HITCH.md");
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].path, boodle);

        // A path outside the project starts at the project.
        let folders = save_folders(&root.join("elsewhere.rs"), &boodle, "HITCH.md");
        assert_eq!(folders[0].path, boodle);
    }

    #[test]
    fn git_file_counts() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".git"), "gitdir: elsewhere").unwrap();
        assert_eq!(
            find_project(tmp.path(), "HITCH.md"),
            Some(tmp.path().to_path_buf())
        );
    }

    #[test]
    fn paths() {
        assert_eq!(normalize(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
        assert_eq!(
            relative_to(Path::new("/p/src/a.rs"), Path::new("/p")),
            Some(PathBuf::from("src/a.rs"))
        );
        assert_eq!(relative_to(Path::new("/q/a.rs"), Path::new("/p")), None);
    }

    #[test]
    fn fallback_names() {
        assert_eq!(app_file_name("org.gnome.Nautilus"), "org.gnome.Nautilus.md");
        assert_eq!(app_file_name("../../etc/passwd"), "_.._etc_passwd.md");
        assert_eq!(
            site_file_name("https://User@Example.com:8443/path?q=1"),
            "example.com_8443.md"
        );
        assert_eq!(
            site_file_name("http://localhost:4000/"),
            "localhost_4000.md"
        );
        assert_eq!(site_file_name("about:blank"), "about_blank.md");
    }
}
