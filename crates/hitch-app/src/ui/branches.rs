//! The clawswap branch page beside the panel.
//!
//! When clawswap's daemon runs the project you're working in, the header
//! shows "Branches". It opens the project's branch page
//! (`http://127.0.0.1:27390/p/<id>`), with its Switch and Pull buttons, in
//! `hitch-web`, a small web view window of its own: iced has no web view,
//! and on Wayland one can't go inside the panel's window. So the window is
//! made to look like a part of the panel instead ([`Look`]): no frame, the
//! panel's colours and header style, the panel's height, and a place
//! against the panel's side. Pressing "Branches" again closes it, as does
//! hiding the panel.
//!
//! `hitch-web` is looked for next to the `hitch` binary (where `cargo build`
//! and Hitch.app put it), then on `PATH`. Without it, the page opens in the
//! default browser.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

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

/// The branch window's width; its height is the panel's.
pub const WIDTH: f32 = 380.0;

/// How the window is made to match the panel.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    /// Keep it above other windows, as the pinned panel is.
    pub pinned: bool,
    pub dark: bool,
    /// The panel's height.
    pub height: f32,
    /// Where the window goes, when the panel knows where it is itself (not
    /// on Wayland, where the GNOME extension docks the window instead).
    pub position: Option<(f32, f32)>,
    /// The panel's background, text and border colours, as `#rrggbb`.
    pub bg: String,
    pub fg: String,
    pub line: String,
}

impl Look {
    /// The place for the window beside a panel at `x`, `y` that is
    /// `panel_width` wide: on its left when there's room, else its right.
    pub fn beside(x: f32, y: f32, panel_width: f32) -> (f32, f32) {
        if x >= WIDTH {
            (x - WIDTH, y)
        } else {
            (x + panel_width, y)
        }
    }

    /// The stylesheet added to clawswap's page: the panel's background and
    /// rule colour for the page's own (its `--bg`, `--card` and `--line`).
    fn page_css(&self) -> String {
        format!(
            ":root{{--bg:{bg};--card:{bg};--line:{line}}}",
            bg = self.bg,
            line = self.line
        )
    }
}

/// `#rrggbb` for a colour's red, green and blue, each from 0 to 1.
pub fn hex(r: f32, g: f32, b: f32) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(r), byte(g), byte(b))
}

/// The open branch page window, if any.
#[derive(Default)]
pub struct BranchWindow {
    child: Mutex<Option<Child>>,
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
    /// Whether the window is open now (it can be closed from its own
    /// header).
    pub fn is_open(&self) -> bool {
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        child
            .as_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    }

    /// Opens `project`'s branch page, closing a window opened before.
    pub fn open(&self, project: &Project, look: &Look) -> std::io::Result<Opened> {
        self.close();
        let url = hitch_context::clawswap::page_url(&ClawswapConfig::from_env(), &project.id);
        let mut command = Command::new(helper_path());
        command
            .args(arguments(project, look, std::process::id()))
            .arg(&url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match command.spawn() {
            Ok(child) => {
                *self.child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
                Ok(Opened::Window)
            }
            Err(e) => {
                super::open_url(&url)?;
                Ok(Opened::Browser(format!("{HELPER} didn't start ({e})")))
            }
        }
    }

    /// Closes the window if it's still open.
    pub fn close(&self) {
        let child = self.child.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(mut child) = child {
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
            }
            // Reaped off the panel's thread.
            std::thread::spawn(move || child.wait());
        }
    }
}

/// `hitch-web`'s options for `project`'s page beside the panel of process
/// `panel_pid`.
fn arguments(project: &Project, look: &Look, panel_pid: u32) -> Vec<String> {
    let mut args = vec![
        "--heading".into(),
        "Branches".into(),
        "--title".into(),
        format!("{} · clawswap", display_name(project)),
        "--size".into(),
        format!("{}x{}", WIDTH, look.height.round()),
        "--dock-to".into(),
        panel_pid.to_string(),
        "--bg".into(),
        look.bg.clone(),
        "--fg".into(),
        look.fg.clone(),
        "--line".into(),
        look.line.clone(),
        "--css".into(),
        look.page_css(),
    ];
    if let Some((x, y)) = look.position {
        args.push("--position".into());
        args.push(format!("{},{}", x.round(), y.round()));
    }
    if look.dark {
        args.push("--dark".into());
    }
    if look.pinned {
        args.push("--pin".into());
    }
    args
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

    fn look() -> Look {
        Look {
            pinned: false,
            dark: false,
            height: 560.4,
            position: None,
            bg: "#f4f4f4".into(),
            fg: "#000000".into(),
            line: "#d9d9d9".into(),
        }
    }

    #[test]
    fn asks_for_the_panels_look() {
        let project = Project {
            id: "hitch".into(),
            name: "hitch".into(),
            root: "/code/hitch".into(),
        };
        let args = arguments(&project, &look(), 4242).join(" ");
        assert_eq!(
            args,
            "--heading Branches --title hitch · clawswap --size 380x560 --dock-to 4242 \
             --bg #f4f4f4 --fg #000000 --line #d9d9d9 \
             --css :root{--bg:#f4f4f4;--card:#f4f4f4;--line:#d9d9d9}"
        );
        let pinned = Look {
            pinned: true,
            dark: true,
            position: Some((1160.0, 48.0)),
            ..look()
        };
        let args = arguments(&project, &pinned, 1).join(" ");
        assert!(args.ends_with("--position 1160,48 --dark --pin"), "{args}");
    }

    #[test]
    fn goes_left_of_the_panel_when_theres_room() {
        assert_eq!(Look::beside(1540.0, 48.0, 320.0), (1160.0, 48.0));
        assert_eq!(Look::beside(8.0, 48.0, 320.0), (328.0, 48.0));
    }

    #[test]
    fn hex_colours() {
        assert_eq!(hex(1.0, 0.5, 0.0), "#ff8000");
        assert_eq!(hex(-1.0, 2.0, 0.956), "#00fff4");
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
