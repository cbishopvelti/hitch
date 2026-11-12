//! The bytes typed into a pane for a `write_chars` message. The rules live
//! in `hitch_protocol::keys`, shared with Hitch's `zellij action
//! write-chars` fallback.

/// Bytes to write into the pane for `text`.
pub fn keystrokes(text: &str, submit: bool) -> Vec<u8> {
    hitch_protocol::keys::keystrokes(text, submit).into_bytes()
}
