//! The context engine.
//!
//! Turns raw signals into the contexts the panel shows (see `docs/SPEC.md`,
//! "What a context is" and "Architecture and platform"):
//!
//! - **Watcher**: the GNOME Shell extension over D-Bus (focused window), the
//!   local socket (Zed and Zellij reports, CLI commands), and clawswap's
//!   `/focus/events` stream. macOS detection comes later.
//! - **Resolver**: applies the ignore list first, then derives contexts from
//!   reports, preferring an exact plugin report while that app is frontmost.
//! - **Debounce**: about 300 ms, so fast switching doesn't flicker.
//!
//! Matching rule 3: when the frontmost app reports no file, the last file
//! context stays current.
//!
//! The app starts an [`Engine`] and gets [`Event`]s from it. Each focus
//! source pushes into an [`InputSink`]: the socket server, the
//! GNOME Shell watcher ([`gnome`]), the [`clawswap`] client, and later macOS
//! detection. The [`resolve::Resolver`] decides what is current.

pub mod clawswap;
mod engine;
#[cfg(target_os = "linux")]
pub mod gnome;
pub mod project;
pub mod resolve;
mod server;
pub mod title;

pub use engine::{Config, Engine, Event, InputSink};
pub use hitch_protocol::{Context, ContextReport, Source};
pub use project::{FsProjectFinder, ProjectFinder};
pub use resolve::{AppTable, IgnoreList, Resolver, WindowFocus};

/// Debounce applied to context changes before the panel updates.
pub const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);

/// The contexts that are current right now, most specific first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActiveContexts {
    pub contexts: Vec<Context>,
}

impl ActiveContexts {
    /// Build from any order; sorts most specific first.
    pub fn new(mut contexts: Vec<Context>) -> Self {
        contexts.sort_by_key(Context::specificity);
        Self { contexts }
    }

    /// The context a new note is hitched to by default.
    pub fn most_specific(&self) -> Option<&Context> {
        self.contexts.first()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_most_specific_first() {
        let a = ActiveContexts::new(vec![
            Context::App {
                app_id: "org.gnome.Nautilus".into(),
            },
            Context::File {
                project: "/p".into(),
                file: "src/lib.rs".into(),
            },
        ]);
        assert!(matches!(a.most_specific(), Some(Context::File { .. })));
    }
}
