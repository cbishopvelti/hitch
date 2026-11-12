//! Paths in window titles, for apps with no Hitch plugin (spec, "Other apps:
//! file path in the window title, when shown").
//!
//! Titles are only parsed here, never stored or logged.

use std::path::{Path, PathBuf};

/// Separators apps put between the document and the app name.
const SEPARATORS: &[&str] = &[" - ", " — ", " – ", " | ", " · ", " • "];

/// Candidate paths in `title`, most likely first. Callers check which exist.
/// `~/` is expanded against `home`.
pub fn candidates(title: &str, home: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut push = |s: &str| {
        if let Some(p) = expand(s, home)
            && !out.contains(&p)
        {
            out.push(p);
        }
    };

    for part in split(title) {
        let part = part.trim_start_matches(['●', '*', '•']).trim();
        // "notes.md (~/Documents)", as GNOME Text Editor and gedit show it.
        if let Some(open) = part.rfind(" (")
            && part.ends_with(')')
        {
            let name = part[..open].trim();
            let dir = &part[open + 2..part.len() - 1];
            if !name.is_empty() && !name.contains('/') && looks_like_path(dir) {
                push(&format!("{}/{name}", dir.trim_end_matches('/')));
            }
        }
        push(part);
        // "user@host: ~/code/hitch" and similar terminal titles.
        for word in part.split_whitespace() {
            push(word.trim_end_matches([':', ',', ';', ')', ']']));
        }
    }
    out
}

fn split(title: &str) -> Vec<&str> {
    let mut parts = vec![title];
    for sep in SEPARATORS {
        parts = parts.into_iter().flat_map(|p| p.split(sep)).collect();
    }
    parts
}

fn looks_like_path(s: &str) -> bool {
    s.starts_with('/') || s == "~" || s.starts_with("~/")
}

fn expand(s: &str, home: Option<&Path>) -> Option<PathBuf> {
    if s.starts_with('/') && s.len() > 1 {
        return Some(PathBuf::from(s));
    }
    let home = home?;
    if s == "~" {
        return Some(home.to_path_buf());
    }
    s.strip_prefix("~/").map(|rest| home.join(rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(title: &str) -> Vec<PathBuf> {
        candidates(title, Some(Path::new("/home/u")))
    }

    #[test]
    fn finds_paths() {
        assert!(c("/etc/hosts - vim").contains(&"/etc/hosts".into()));
        assert_eq!(
            c("notes.md (~/Documents) - Text Editor")[0],
            PathBuf::from("/home/u/Documents/notes.md")
        );
        assert!(c("u@box: ~/code/hitch").contains(&"/home/u/code/hitch".into()));
        assert!(c("● ~/x.rs — Editor").contains(&"/home/u/x.rs".into()));
        assert!(c("Slack | general").is_empty());
        assert!(c("a / b").is_empty());
    }

    #[test]
    fn no_home_no_tilde() {
        assert!(candidates("~/x", None).is_empty());
    }
}
