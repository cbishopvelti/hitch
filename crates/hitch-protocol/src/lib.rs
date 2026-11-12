//! Shared types for Hitch and its plugins.
//!
//! This crate is the contract between the Hitch app and everything that
//! reports context to it (the Zed extension, the Zellij plugin, the `hitch`
//! CLI). It has no dependencies beyond `serde`, so it builds for the wasm
//! targets the plugins use.
//!
//! - [`context`]: what a context is, after resolution.
//! - [`message`]: the messages sent over the local socket.
//! - [`frame`]: newline-delimited JSON framing.
//! - [`paths`]: where the socket lives.
//! - [`zed`]: reading Zed's active file from its window title.
//! - [`keys`]: the keystrokes a `write_chars` types into a Zellij pane.
//!
//! The wire format is documented for non-Rust clients in `docs/PROTOCOL.md`.

pub mod context;
pub mod frame;
pub mod keys;
pub mod message;
pub mod paths;
pub mod zed;

#[cfg(unix)]
pub mod client;

pub use context::{Context, ContextReport, Source};
pub use message::{ClientMessage, Command, ServerMessage};

/// Version of the socket protocol. Bumped only for breaking changes; adding
/// an optional field or a new message type is not breaking.
pub const PROTOCOL_VERSION: u32 = 1;

/// What the Zellij plugin prints on `zellij pipe`'s stdout once it has
/// typed a `write_chars`. With no plugin listening, `zellij pipe` with its
/// stdin closed exits 0 and prints nothing, so the exit code alone doesn't
/// say the text was typed.
pub const ZELLIJ_PIPE_ACK: &str = "typed";

/// The Wayland app id and D-Bus prefix used by Hitch.
pub const APP_ID: &str = "io.github.cbishopvelti.Hitch";
