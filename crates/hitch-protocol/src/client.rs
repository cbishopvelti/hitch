//! A minimal blocking client, for the CLI and native plugins.

use std::io::{self, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use crate::frame;
use crate::message::ClientMessage;

/// Connect to the socket at `path`, send `msgs`, and close.
pub fn send_all(path: &Path, msgs: &[ClientMessage]) -> io::Result<()> {
    let mut stream = UnixStream::connect(path)?;
    for msg in msgs {
        let line = frame::encode(msg).map_err(io::Error::other)?;
        stream.write_all(line.as_bytes())?;
    }
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Command;
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;

    #[test]
    fn sends_lines_to_listener() {
        let dir = std::env::temp_dir().join(format!("hitch-proto-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("t.sock");
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).unwrap();

        let msg = ClientMessage::Command {
            command: Command::TogglePanel,
        };
        let expected = msg.clone();
        let reader = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(s).read_line(&mut line).unwrap();
            frame::decode::<ClientMessage>(&line).unwrap()
        });
        send_all(&sock, &[msg]).unwrap();
        assert_eq!(reader.join().unwrap(), expected);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
