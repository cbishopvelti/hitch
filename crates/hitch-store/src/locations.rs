//! The known-locations list, `~/.hitch/locations.json`: the path of every
//! notes file Hitch has seen, so search and the file watcher can cover
//! projects that aren't open right now.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{StoreError, write_atomic};

#[derive(Debug, Default, Serialize, Deserialize)]
struct OnDisk {
    #[serde(default)]
    files: BTreeSet<PathBuf>,
}

/// The known notes files, backed by a JSON file.
#[derive(Debug)]
pub struct Locations {
    path: PathBuf,
    files: BTreeSet<PathBuf>,
}

impl Locations {
    /// Load the list, or start an empty one if the file is missing or
    /// unreadable (it is only a cache of paths).
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let files = read(&path).unwrap_or_default();
        Self { path, files }
    }

    /// The known notes files, sorted.
    pub fn files(&self) -> impl Iterator<Item = &Path> {
        self.files.iter().map(PathBuf::as_path)
    }

    /// Whether `file` is on the list.
    pub fn contains(&self, file: &Path) -> bool {
        self.files.contains(file)
    }

    /// Add a notes file and save the list if it is new. Re-reads the file
    /// first, so entries added by another Hitch process (the CLI) are kept.
    pub fn add(&mut self, file: &Path) -> Result<bool, StoreError> {
        if self.files.contains(file) {
            return Ok(false);
        }
        self.files.extend(read(&self.path).unwrap_or_default());
        let added = self.files.insert(file.to_path_buf());
        if added {
            self.save()?;
        }
        Ok(added)
    }

    /// Drop entries whose file no longer exists, and save if any were.
    pub fn prune(&mut self) -> Result<Vec<PathBuf>, StoreError> {
        let gone: Vec<PathBuf> = self
            .files
            .iter()
            .filter(|f| !f.is_file())
            .cloned()
            .collect();
        if !gone.is_empty() {
            for f in &gone {
                self.files.remove(f);
            }
            self.save()?;
        }
        Ok(gone)
    }

    fn save(&self) -> Result<(), StoreError> {
        let on_disk = OnDisk {
            files: self.files.clone(),
        };
        let mut json = serde_json::to_string_pretty(&on_disk)?;
        json.push('\n');
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| StoreError::io(dir, e))?;
        }
        write_atomic(&self.path, &json)
    }
}

fn read(path: &Path) -> Option<BTreeSet<PathBuf>> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<OnDisk>(&text).ok().map(|d| d.files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_merges_and_prunes() {
        let tmp = tempfile::tempdir().unwrap();
        let list = tmp.path().join("hitch").join("locations.json");
        let a = tmp.path().join("a.md");
        let b = tmp.path().join("b.md");
        std::fs::write(&a, "").unwrap();
        std::fs::write(&b, "").unwrap();

        let mut one = Locations::load(&list);
        let mut two = Locations::load(&list);
        assert!(one.add(&a).unwrap());
        assert!(!one.add(&a).unwrap());
        assert!(two.add(&b).unwrap());
        let both = Locations::load(&list);
        assert_eq!(
            both.files().count(),
            2,
            "the second writer kept the first's entry"
        );

        std::fs::remove_file(&a).unwrap();
        let mut again = Locations::load(&list);
        assert_eq!(again.prune().unwrap(), vec![a.clone()]);
        assert!(!Locations::load(&list).contains(&a));
    }
}
