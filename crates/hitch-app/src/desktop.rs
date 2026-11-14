//! Linux desktop integration: the `.desktop` entry and icon that GNOME (and
//! other freedesktop desktops) use for Hitch's window in Alt+Tab, the dock
//! and the app grid.
//!
//! A Wayland window has no icon of its own: the shell looks up the
//! `.desktop` file named after the window's app ID
//! ([`hitch_protocol::APP_ID`]), and on X11 it matches `StartupWMClass`.
//! Hitch writes both files under `$XDG_DATA_HOME` (`~/.local/share`) when it
//! starts, so a `cargo run` build gets its icon with no install step. They
//! are only rewritten when their contents change, e.g. the binary moved.

use std::io;
use std::path::{Path, PathBuf};

use hitch_protocol::APP_ID;

const DESKTOP: &str = include_str!("../../../data/io.github.cbishopvelti.Hitch.desktop");
const ICON: &str = include_str!("../../../data/io.github.cbishopvelti.Hitch.svg");

/// Writes the `.desktop` entry and icon if they are missing or out of date.
pub fn install() -> io::Result<()> {
    let data = data_home().ok_or_else(|| io::Error::other("HOME is not set"))?;
    let exe = std::env::current_exe()?;
    install_into(&data, &exe)
}

fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
}

fn install_into(data: &Path, exe: &Path) -> io::Result<()> {
    write_if_changed(
        &data.join(format!("icons/hicolor/scalable/apps/{APP_ID}.svg")),
        ICON,
    )?;
    write_if_changed(
        &data.join(format!("applications/{APP_ID}.desktop")),
        &desktop_entry(exe),
    )
}

/// The entry, launching this binary.
fn desktop_entry(exe: &Path) -> String {
    DESKTOP.replace("\nExec=hitch\n", &format!("\nExec={}\n", exec_arg(exe)))
}

/// Quotes a path for `Exec=` as the Desktop Entry spec asks.
fn exec_arg(exe: &Path) -> String {
    let path = exe.to_string_lossy();
    if !path.contains(|c: char| c.is_whitespace() || "\"'\\><~|&;$*?#()`%".contains(c)) {
        return path.into_owned();
    }
    let mut quoted = String::from("\"");
    for c in path.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

fn write_if_changed(path: &Path, contents: &str) -> io::Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|old| old == contents) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Write then rename, so the shell never reads half a file.
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_names_the_app_id_and_this_binary() {
        let entry = desktop_entry(Path::new("/home/me/hitch/target/debug/hitch"));
        assert!(entry.contains("\nExec=/home/me/hitch/target/debug/hitch\n"));
        assert!(entry.contains(&format!("\nIcon={APP_ID}\n")));
        assert!(entry.contains(&format!("\nStartupWMClass={APP_ID}\n")));
    }

    #[test]
    fn exec_paths_with_spaces_are_quoted() {
        assert_eq!(exec_arg(Path::new("/a b/hitch")), "\"/a b/hitch\"");
        assert_eq!(exec_arg(Path::new("/a b/50%")), "\"/a b/50%%\"");
    }

    #[test]
    fn installs_and_leaves_unchanged_files_alone() {
        let dir = std::env::temp_dir().join(format!("hitch-desktop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        install_into(&dir, Path::new("/opt/hitch")).unwrap();
        let entry = dir.join(format!("applications/{APP_ID}.desktop"));
        let icon = dir.join(format!("icons/hicolor/scalable/apps/{APP_ID}.svg"));
        assert!(
            std::fs::read_to_string(&entry)
                .unwrap()
                .contains("Exec=/opt/hitch\n")
        );
        assert_eq!(std::fs::read_to_string(&icon).unwrap(), ICON);

        let before = std::fs::metadata(&icon).unwrap().modified().unwrap();
        install_into(&dir, Path::new("/usr/bin/hitch")).unwrap();
        assert!(
            std::fs::read_to_string(&entry)
                .unwrap()
                .contains("Exec=/usr/bin/hitch\n")
        );
        assert_eq!(
            std::fs::metadata(&icon).unwrap().modified().unwrap(),
            before
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
