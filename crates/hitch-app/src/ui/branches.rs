//! The clawswap branch page beside the panel.
//!
//! When clawswap's daemon runs the project you're working in, the header
//! shows "Branches". It opens the project's branch page
//! (`http://127.0.0.1:27390/p/<id>`), with its Switch and Pull buttons, in
//! `hitch-web`, a small web view window of its own: iced has no web view,
//! and on Wayland one can't go inside the panel's window. Pressing it again
//! replaces that window, so it always shows the project now current.
//!
//! `hitch-web` is looked for next to the `hitch` binary (where `cargo build`
//! and Hitch.app put it), then on `PATH`. Without it, the page opens in the
//! default browser.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use hitch_context::clawswap::{ClawswapConfig, Project};
use hitch_protocol::Context;

const HELPER: &str = "hitch-web";

/// The folders the active contexts are in, for matching to a clawswap
/// project.
pub fn folders(contexts: &[Context]) -> impl Iterator<Item = &Path> {
    contexts.iter().filter_map(|c| match c {
        Context::File { project, .. } => Some(project.as_path()),
        Context::Folder { path } => Some(path.as_path()),
        Context::Url { .. } | Context::App { .. } => None,
    })
}

/// The open branch page window, if any.
#[derive(Default)]
pub struct BranchWindow {
    child: Option<Child>,
}

/// How the page was opened.
#[derive(Debug, PartialEq, Eq)]
pub enum Opened {
    /// In `hitch-web`.
    Window,
    /// In the default browser, because `hitch-web` couldn't start.
    Browser(String),
}

impl BranchWindow {
    /// Opens `project`'s branch page, closing a window opened before.
    pub fn open(&mut self, project: &Project, pinned: bool) -> std::io::Result<Opened> {
        self.close();
        let url = hitch_context::clawswap::page_url(&ClawswapConfig::from_env(), &project.id);
        let title = format!("{} · clawswap", display_name(project));
        let mut command = Command::new(helper_path());
        command.arg("--title").arg(&title);
        if pinned {
            command.arg("--pin");
        }
        command
            .arg(&url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match command.spawn() {
            Ok(child) => {
                self.child = Some(child);
                Ok(Opened::Window)
            }
            Err(e) => {
                super::open_url(&url)?;
                Ok(Opened::Browser(format!("{HELPER} didn't start ({e})")))
            }
        }
    }

    /// Closes the window if it's still open.
    pub fn close(&mut self) {
        if let Some(mut child) = self.child.take() {
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
            }
            // Reaped off the panel's thread.
            std::thread::spawn(move || child.wait());
        }
    }
}

impl Drop for BranchWindow {
    fn drop(&mut self) {
        self.close();
    }
}

fn display_name(project: &Project) -> &str {
    if project.name.is_empty() {
        &project.id
    } else {
        &project.name
    }
}

/// `hitch-web` beside this executable, else the bare name for `PATH`.
fn helper_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name(HELPER))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(HELPER))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_of_file_and_folder_contexts() {
        let contexts = [
            Context::File {
                project: "/code/hitch".into(),
                file: "src/main.rs".into(),
            },
            Context::Folder {
                path: "/code/shop".into(),
            },
            Context::Url {
                url: "https://example.com".into(),
            },
            Context::App {
                app_id: "org.gnome.Nautilus".into(),
            },
        ];
        assert_eq!(
            folders(&contexts).collect::<Vec<_>>(),
            [Path::new("/code/hitch"), Path::new("/code/shop")]
        );
    }

    #[test]
    fn titles_by_name_then_id() {
        let mut p = Project {
            id: "hitch-2".into(),
            name: "hitch".into(),
            root: "/code/hitch".into(),
        };
        assert_eq!(display_name(&p), "hitch");
        p.name.clear();
        assert_eq!(display_name(&p), "hitch-2");
    }
}
