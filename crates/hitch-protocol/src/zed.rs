//! Reading Zed's context from its window title.
//!
//! Zed's extension API (`zed_extension_api` 0.7.0) has no way to see the
//! active editor, its file, or window focus: extensions only answer calls
//! for language servers, slash commands, context servers and debug adapters.
//! So Hitch gets Zed's context from the window title instead, which the GNOME
//! Shell extension sends with every `FocusChanged` (title changes included).
//!
//! Zed 1.20 and later let the user template the title. With the setting in
//! [`RECOMMENDED_SETTINGS`], the title carries the active file's absolute
//! path:
//!
//! ```text
//! hitch — /home/u/code/hitch/src/main.rs
//! ```
//!
//! Zed's default title (`hitch — main.rs`) only has the file name, which
//! [`parse_window_title`] reports as [`ZedTitle::FileName`] so Hitch can
//! suggest the setting. See `integrations/zed/README.md`.

use std::path::{Path, PathBuf};

use crate::context::{ContextReport, Source};

/// Wayland app ids (and desktop file ids) of Zed's release channels.
pub const APP_IDS: [&str; 4] = [
    "dev.zed.Zed",
    "dev.zed.Zed-Preview",
    "dev.zed.Zed-Nightly",
    "dev.zed.Zed-Dev",
];

/// The `window_title_format` value Hitch asks Zed users to set.
pub const WINDOW_TITLE_FORMAT: &str = "${projectName}${separator}${filePath}";

/// The settings to add to `~/.config/zed/settings.json`.
pub const RECOMMENDED_SETTINGS: &str =
    r#""window_title_format": "${projectName}${separator}${filePath}""#;

/// Zed's default `window_title_separator`.
pub const DEFAULT_SEPARATOR: &str = " — ";

/// Markers Zed appends to the title of a shared (`↗`) or joined (`↙`)
/// collaboration project.
const COLLAB_MARKERS: [&str; 2] = [" ↗", " ↙"];

/// True if `app_id` (a desktop file id, Wayland app id or WM_CLASS) is Zed.
pub fn is_zed_app_id(app_id: &str) -> bool {
    let id = app_id.strip_suffix(".desktop").unwrap_or(app_id);
    APP_IDS.iter().any(|z| z.eq_ignore_ascii_case(id)) || id.eq_ignore_ascii_case("zed")
}

/// What a Zed window title says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZedTitle {
    /// The title has the active file's absolute path (the recommended
    /// `window_title_format`). `project_name` is Zed's project name, the
    /// worktree folder names joined by ", ".
    File {
        project_name: Option<String>,
        path: PathBuf,
    },
    /// Zed's default title: the project name and the active file's name,
    /// but no path. Hitch can't resolve this to a file.
    FileName {
        project_name: String,
        file_name: String,
    },
    /// No file is active (a terminal, the agent panel, an empty pane).
    Project { project_name: String },
}

impl ZedTitle {
    /// The active file, when the title has its path.
    pub fn file(&self) -> Option<&Path> {
        match self {
            ZedTitle::File { path, .. } => Some(path),
            _ => None,
        }
    }

    /// The report Hitch would get if Zed could send one: the active file
    /// with `source: zed`. `None` when the title has no path.
    pub fn report(&self) -> Option<ContextReport> {
        self.file().map(|file| ContextReport {
            source: Source::Zed,
            file: Some(file.to_path_buf()),
            ..Default::default()
        })
    }
}

/// Parse the title of a Zed window. `None` for an empty title.
///
/// Project names are folder names, which can't contain `/`, so the first
/// `/` starts the file path whatever separator the user chose. A title with
/// no `/` is Zed's default format, split on [`DEFAULT_SEPARATOR`].
pub fn parse_window_title(title: &str) -> Option<ZedTitle> {
    let mut title = title.trim();
    for marker in COLLAB_MARKERS {
        if let Some(rest) = title.strip_suffix(marker) {
            title = rest.trim_end();
        }
    }
    if title.is_empty() {
        return None;
    }

    if let Some(slash) = title.find('/') {
        let (prefix, path) = title.split_at(slash);
        let project_name = prefix
            .trim_end_matches(|c: char| c.is_whitespace() || "—–-|:·•".contains(c))
            .to_string();
        return Some(ZedTitle::File {
            project_name: (!project_name.is_empty()).then_some(project_name),
            path: PathBuf::from(path),
        });
    }

    match title.rsplit_once(DEFAULT_SEPARATOR) {
        Some((project, file)) if !project.is_empty() && !file.is_empty() => {
            Some(ZedTitle::FileName {
                project_name: project.to_string(),
                file_name: file.to_string(),
            })
        }
        _ => Some(ZedTitle::Project {
            project_name: title.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(project: Option<&str>, path: &str) -> Option<ZedTitle> {
        Some(ZedTitle::File {
            project_name: project.map(str::to_string),
            path: PathBuf::from(path),
        })
    }

    #[test]
    fn recommended_format() {
        assert_eq!(
            parse_window_title("hitch — /home/u/code/hitch/src/main.rs"),
            file(Some("hitch"), "/home/u/code/hitch/src/main.rs")
        );
    }

    #[test]
    fn several_worktrees_and_spaces_in_paths() {
        assert_eq!(
            parse_window_title("hitch, clawswap — /home/u/My Code/clawswap/lib/a — b.ex"),
            file(
                Some("hitch, clawswap"),
                "/home/u/My Code/clawswap/lib/a — b.ex"
            )
        );
    }

    #[test]
    fn custom_separator_or_path_only() {
        assert_eq!(
            parse_window_title("hitch | /p/src/lib.rs"),
            file(Some("hitch"), "/p/src/lib.rs")
        );
        assert_eq!(
            parse_window_title("/p/src/lib.rs"),
            file(None, "/p/src/lib.rs")
        );
    }

    #[test]
    fn collaboration_marker_is_dropped() {
        assert_eq!(
            parse_window_title("hitch — /p/a.rs ↗"),
            file(Some("hitch"), "/p/a.rs")
        );
        assert_eq!(
            parse_window_title("hitch — main.rs ↙"),
            Some(ZedTitle::FileName {
                project_name: "hitch".into(),
                file_name: "main.rs".into()
            })
        );
    }

    #[test]
    fn default_format_and_no_file() {
        assert_eq!(
            parse_window_title("hitch — main.rs"),
            Some(ZedTitle::FileName {
                project_name: "hitch".into(),
                file_name: "main.rs".into()
            })
        );
        assert_eq!(
            parse_window_title("hitch"),
            Some(ZedTitle::Project {
                project_name: "hitch".into()
            })
        );
        assert_eq!(parse_window_title("  "), None);
    }

    #[test]
    fn report_has_the_file() {
        let r = parse_window_title("hitch — /p/a.rs")
            .unwrap()
            .report()
            .unwrap();
        assert_eq!(r.source, Source::Zed);
        assert_eq!(r.file.as_deref(), Some(Path::new("/p/a.rs")));
        assert!(
            parse_window_title("hitch — a.rs")
                .unwrap()
                .report()
                .is_none()
        );
    }

    #[test]
    fn app_ids() {
        assert!(is_zed_app_id("dev.zed.Zed"));
        assert!(is_zed_app_id("dev.zed.Zed-Preview.desktop"));
        assert!(is_zed_app_id("zed"));
        assert!(!is_zed_app_id("org.gnome.Terminal"));
    }

    #[test]
    fn settings_snippet_matches_format() {
        assert!(RECOMMENDED_SETTINGS.contains(WINDOW_TITLE_FORMAT));
    }
}
