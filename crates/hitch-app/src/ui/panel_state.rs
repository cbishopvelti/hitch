//! Panel state that survives restarts: whether the panel is pinned, the
//! screen edge it docks to, its size, and the last "Save to" choice per
//! project.
//!
//! Kept in `~/.hitch/panel.json`, apart from `settings.json`, because it is
//! state the panel changes as you use it rather than a preference you edit.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The screen edge the panel docks to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Left,
    #[default]
    Right,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelState {
    /// Always on top, on every workspace, until unpinned (spec, UX).
    pub pinned: bool,
    pub edge: Edge,
    /// The size the panel was last resized to; the default size if none.
    pub size: Option<PanelSize>,
    /// The last "Save to" choice per project (spec F14), keyed by the
    /// project's notes file. Only choices other than the default are kept.
    pub save_to: BTreeMap<String, SavedChoice>,
}

/// The panel's size in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelSize {
    pub width: u32,
    pub height: u32,
}

/// A remembered "Save to" choice: the notes file, and whether the note is
/// hitched to the file being edited or saved for the whole folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedChoice {
    pub path: PathBuf,
    pub file: bool,
}

impl PanelState {
    /// `~/.hitch/panel.json`.
    pub fn default_path() -> Option<PathBuf> {
        hitch_protocol::paths::hitch_dir().map(|dir| dir.join("panel.json"))
    }

    /// Reads the state, falling back to the defaults if the file is missing
    /// or unreadable: losing the pin is better than refusing to start.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Writes the state atomically, creating `~/.hitch` (mode 0700) if
    /// needed.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            create_private_dir(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_defaults() {
        let dir = std::env::temp_dir().join(format!("hitch-panel-state-{}", std::process::id()));
        let path = dir.join("nested").join("panel.json");

        assert_eq!(PanelState::load(&path), PanelState::default());

        let mut state = PanelState {
            pinned: true,
            edge: Edge::Left,
            size: Some(PanelSize {
                width: 400,
                height: 700,
            }),
            ..PanelState::default()
        };
        state.save_to.insert(
            "/p/HITCH.md".into(),
            SavedChoice {
                path: "/HITCH.md".into(),
                file: true,
            },
        );
        state.save(&path).unwrap();
        assert_eq!(PanelState::load(&path), state);

        // Unknown or missing fields don't lose the rest.
        std::fs::write(&path, r#"{"pinned":true,"from_the_future":1}"#).unwrap();
        assert_eq!(
            PanelState::load(&path),
            PanelState {
                pinned: true,
                ..PanelState::default()
            }
        );

        std::fs::write(&path, "not json").unwrap();
        assert_eq!(PanelState::load(&path), PanelState::default());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
