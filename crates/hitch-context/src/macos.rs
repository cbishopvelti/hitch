//! macOS focus and global shortcuts (spec, "Architecture and platform",
//! macOS column), wired into the engine. The Cocoa and Accessibility code is
//! in the `hitch-macos` crate; this only adapts it.

use hitch_macos::{Focus, FocusEvent, FocusWatcher, Hotkeys};
use hitch_protocol::{ClientMessage, Command};

use crate::engine::InputSink;
use crate::resolve::WindowFocus;

/// Holds the watcher and shortcuts for as long as the engine lives.
pub(crate) struct Mac {
    _focus: Option<FocusWatcher>,
    _hotkeys: Option<Hotkeys>,
}

impl Mac {
    pub(crate) fn start(
        sink: &InputSink,
        watch_focus: bool,
        request_accessibility: bool,
        shortcuts: &[(String, Command)],
    ) -> Self {
        let focus = watch_focus.then(|| {
            if request_accessibility {
                // Shows macOS's prompt if Hitch isn't trusted yet; macOS only
                // shows it once per app.
                hitch_macos::request_accessibility();
            }
            let s = sink.clone();
            FocusWatcher::start(move |event| match event {
                FocusEvent::Focus(f) => s.window_focus(window_focus(f)),
                FocusEvent::Accessibility(trusted) => s.accessibility(trusted),
            })
        });

        let actions: Vec<(String, Box<dyn Fn() + Send + Sync>)> = shortcuts
            .iter()
            .map(|(keys, command)| {
                let s = sink.clone();
                let command = *command;
                let action: Box<dyn Fn() + Send + Sync> =
                    Box::new(move || s.message(ClientMessage::Command { command }));
                (keys.clone(), action)
            })
            .collect();
        let s = sink.clone();
        let hotkeys = if actions.is_empty() {
            None
        } else {
            match Hotkeys::register(actions, move |keys| s.shortcut_unavailable(keys)) {
                Ok(h) => Some(h),
                Err(_) => {
                    for (keys, _) in shortcuts {
                        sink.shortcut_unavailable(keys.clone());
                    }
                    None
                }
            }
        };

        Self {
            _focus: focus,
            _hotkeys: hotkeys,
        }
    }
}

/// The bundle id plays the app id, and the app's name the WM class, so
/// patterns like `zed` or `kitty` in the app table and ignore list match on
/// macOS too.
fn window_focus(f: Focus) -> WindowFocus {
    WindowFocus {
        app_id: f.bundle_id,
        wm_class: f.name,
        title: f.title,
        pid: u32::try_from(f.pid).unwrap_or(0),
        sandboxed_app_id: String::new(),
        document: f.document,
    }
}
