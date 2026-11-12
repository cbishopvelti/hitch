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
