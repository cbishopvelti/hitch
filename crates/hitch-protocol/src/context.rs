//! Context types.
//!
//! A [`ContextReport`] is what a source tells Hitch ("Zed has src/main.rs open
//! in ~/code/hitch"). The context engine turns reports into [`Context`]s,
//! which the store uses to find notes.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Where a report came from. Plugin reports are exact, so they win over OS
/// detection while their app is frontmost.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The Hitch extension for Zed.
    Zed,
    /// The Hitch plugin for Zellij.
    Zellij,
    /// clawswap's focus events (a dev-server page in the browser).
    Clawswap,
    /// The GNOME Shell extension (focused window on Wayland).
    GnomeShell,
    /// macOS frontmost-app detection.
    Macos,
    /// The `hitch` command line.
    Cli,
    /// Any other source. Lets new plugins report before this enum knows them.
    #[serde(untagged)]
    Other(String),
}

/// A context as reported by a source. Every field except `source` is
/// optional; a source fills in what it knows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextReport {
    pub source: Source,
    /// Project root as the source sees it (Zed worktree, clawswap root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<PathBuf>,
    /// Absolute path of the active file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// Working folder (a Zellij pane's cwd).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// Page URL, for sources that know one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Source-specific session name (the Zellij session).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// Source-specific pane or window id (the Zellij pane id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
}

impl Default for Source {
    fn default() -> Self {
        Source::Other(String::new())
    }
}

/// A resolved context, most specific first in [`Context::specificity`] order.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Context {
    /// A file inside a project folder. `file` is relative to `project`.
    File { project: PathBuf, file: PathBuf },
    /// A project folder, the one whose HITCH.md holds the notes.
    Folder { path: PathBuf },
    /// A web page or site with no project behind it.
    Url { url: String },
    /// An app with no file, identified by its desktop app id.
    App { app_id: String },
}

impl Context {
    /// Lower is more specific. The panel sorts by this.
    pub fn specificity(&self) -> u8 {
        match self {
            Context::File { .. } => 0,
            Context::Folder { .. } => 1,
            Context::Url { .. } => 2,
            Context::App { .. } => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_source_round_trips() {
        let s: Source = serde_json::from_str("\"vscode\"").unwrap();
        assert_eq!(s, Source::Other("vscode".into()));
        assert_eq!(serde_json::to_string(&s).unwrap(), "\"vscode\"");
        let z: Source = serde_json::from_str("\"gnome_shell\"").unwrap();
        assert_eq!(z, Source::GnomeShell);
    }

    #[test]
    fn context_is_tagged() {
        let c = Context::Folder {
            path: "/home/u/code/hitch".into(),
        };
        assert_eq!(
            serde_json::to_string(&c).unwrap(),
            r#"{"kind":"folder","path":"/home/u/code/hitch"}"#
        );
        assert!(
            Context::File {
                project: "/p".into(),
                file: "a.rs".into()
            }
            .specificity()
                < c.specificity()
        );
    }
}
