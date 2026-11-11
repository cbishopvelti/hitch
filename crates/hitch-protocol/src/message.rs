//! Messages on the Hitch socket.
//!
//! Each message is a JSON object with a `type` field. Receivers ignore
//! message types and fields they don't know, so either side can add them
//! without a protocol version bump.

use serde::{Deserialize, Serialize};

use crate::context::{ContextReport, Source};

/// Sent by a plugin or the CLI to Hitch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// First message on a long-lived connection.
    Hello {
        /// Client name, e.g. "hitch-zed".
        client: String,
        /// Client version, free form.
        version: String,
        /// Protocol version the client speaks.
        protocol: u32,
        /// Server messages this client can act on, e.g. `["write_chars"]`.
        #[serde(default)]
        capabilities: Vec<String>,
    },
    /// The source's context changed: a new active file, a new focused pane.
    /// Sent whenever it changes while the source is focused, and once when
    /// the source gains focus.
    Context(ContextReport),
    /// The source lost focus or closed. Hitch stops preferring its last
    /// report but keeps showing its notes (spec, matching rule 3).
    Blur { source: Source },
    /// A command from the `hitch` CLI or a shortcut.
    Command { command: Command },
    /// A message type this version doesn't know. Ignored.
    #[serde(other)]
    Unknown,
}

/// Commands that drive the panel. `hitch <command>` sends these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    /// Show or hide the panel (default Ctrl+Shift+N).
    TogglePanel,
    ShowPanel,
    HidePanel,
    /// Open quick capture for the current context (default Ctrl+Shift+H).
    Capture,
    /// Pin or unpin the panel always on top (default Ctrl+Shift+P).
    TogglePin,
    /// Open search.
    Search,
}

/// Sent by Hitch to a connected client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Reply to `hello`.
    Welcome {
        protocol: u32,
        /// Hitch's version.
        version: String,
    },
    /// Type `text` into the last focused pane (Zellij). With `submit`, also
    /// press Enter. Sent only to clients that listed `write_chars` in their
    /// capabilities.
    WriteChars { text: String, submit: bool },
    /// The last message could not be handled.
    Error { message: String },
    /// A message type this version doesn't know. Ignored.
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn context_report_wire_shape() {
        let m = ClientMessage::Context(ContextReport {
            source: Source::Zed,
            project_root: Some(PathBuf::from("/home/u/code/hitch")),
            file: Some(PathBuf::from("/home/u/code/hitch/src/main.rs")),
            ..Default::default()
        });
        assert_eq!(
            serde_json::to_string(&m).unwrap(),
            r#"{"type":"context","source":"zed","project_root":"/home/u/code/hitch","file":"/home/u/code/hitch/src/main.rs"}"#
        );
    }

    #[test]
    fn parses_documented_examples() {
        let hello: ClientMessage = serde_json::from_str(
            r#"{"type":"hello","client":"hitch-zellij","version":"0.1.0","protocol":1,"capabilities":["write_chars"]}"#,
        )
        .unwrap();
        assert!(matches!(hello, ClientMessage::Hello { protocol: 1, .. }));

        let cmd: ClientMessage =
            serde_json::from_str(r#"{"type":"command","command":"toggle_panel"}"#).unwrap();
        assert_eq!(
            cmd,
            ClientMessage::Command {
                command: Command::TogglePanel
            }
        );

        let w: ServerMessage =
            serde_json::from_str(r#"{"type":"write_chars","text":"cargo run","submit":false}"#)
                .unwrap();
        assert_eq!(
            w,
            ServerMessage::WriteChars {
                text: "cargo run".into(),
                submit: false
            }
        );
    }

    #[test]
    fn unknown_types_and_fields_are_ignored() {
        let m: ClientMessage = serde_json::from_str(r#"{"type":"from_the_future","x":1}"#).unwrap();
        assert_eq!(m, ClientMessage::Unknown);
        let m: ClientMessage =
            serde_json::from_str(r#"{"type":"blur","source":"zellij","extra":true}"#).unwrap();
        assert_eq!(
            m,
            ClientMessage::Blur {
                source: Source::Zellij
            }
        );
    }
}
