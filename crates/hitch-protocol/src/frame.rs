//! Framing: one JSON object per line, UTF-8, `\n` terminated.

use serde::{Serialize, de::DeserializeOwned};

/// Longest line a receiver must accept, in bytes, newline included. Longer
/// lines may be dropped and the connection closed.
pub const MAX_LINE_BYTES: usize = 64 * 1024;

/// Encode a message as one line, newline included.
pub fn encode<T: Serialize>(msg: &T) -> serde_json::Result<String> {
    let mut line = serde_json::to_string(msg)?;
    line.push('\n');
    Ok(line)
}

/// Decode one line. Trailing `\r\n` or `\n` is ignored.
pub fn decode<T: DeserializeOwned>(line: &str) -> serde_json::Result<T> {
    serde_json::from_str(line.trim_end_matches(['\r', '\n']))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ClientMessage, Command};

    #[test]
    fn round_trip() {
        let m = ClientMessage::Command {
            command: Command::Capture,
        };
        let line = encode(&m).unwrap();
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);
        assert_eq!(decode::<ClientMessage>(&line).unwrap(), m);
    }
}
