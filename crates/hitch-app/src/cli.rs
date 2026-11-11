//! Command line parsing and the commands that talk to a running Hitch.

use std::io::{self, BufRead};

use hitch_protocol::{ClientMessage, Command, client, frame, paths};

pub const USAGE: &str = "\
usage: hitch [command]

  (none)    start Hitch
  toggle    show or hide the panel
  show      show the panel
  hide      hide the panel
  capture   quick capture for the current context
  pin       pin or unpin the panel always on top
  search    open search
  send [JSON...]
            forward messages to Hitch: each argument is one JSON message;
            with no arguments, one message per line of stdin
";

#[derive(Debug, PartialEq, Eq)]
pub enum Invocation {
    RunApp,
    Help,
    Command(Command),
    /// Messages given as arguments; empty means read stdin.
    Send(Vec<String>),
}

pub fn parse(args: &[String]) -> Result<Invocation, String> {
    let Some(first) = args.first() else {
        return Ok(Invocation::RunApp);
    };
    if first == "send" {
        return Ok(Invocation::Send(args[1..].to_vec()));
    }
    if args.len() > 1 {
        return Err(format!("unexpected argument '{}'", args[1]));
    }
    Ok(match first.as_str() {
        "toggle" => Invocation::Command(Command::TogglePanel),
        "show" => Invocation::Command(Command::ShowPanel),
        "hide" => Invocation::Command(Command::HidePanel),
        "capture" => Invocation::Command(Command::Capture),
        "pin" => Invocation::Command(Command::TogglePin),
        "search" => Invocation::Command(Command::Search),
        "-h" | "--help" | "help" => Invocation::Help,
        other => return Err(format!("unknown command '{other}'")),
    })
}

pub fn execute(inv: Invocation) -> io::Result<()> {
    let sock = paths::socket_path()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    let msgs = match inv {
        Invocation::Command(command) => vec![ClientMessage::Command { command }],
        Invocation::Send(lines) if lines.is_empty() => read_messages(io::stdin().lock())?,
        Invocation::Send(args) => args.iter().map(|a| decode(a)).collect::<io::Result<_>>()?,
        Invocation::RunApp | Invocation::Help => return Ok(()),
    };
    client::send_all(&sock, &msgs).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("can't reach Hitch at {}: {e}", sock.display()),
        )
    })
}

/// Read and validate one message per non-empty line.
fn read_messages(input: impl BufRead) -> io::Result<Vec<ClientMessage>> {
    let mut msgs = Vec::new();
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let msg =
            frame::decode(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        msgs.push(msg);
    }
    Ok(msgs)
}

fn decode(json: &str) -> io::Result<ClientMessage> {
    frame::decode(json).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_commands() {
        assert_eq!(parse(&[]).unwrap(), Invocation::RunApp);
        assert_eq!(
            parse(&args(&["toggle"])).unwrap(),
            Invocation::Command(Command::TogglePanel)
        );
        assert_eq!(parse(&args(&["send"])).unwrap(), Invocation::Send(vec![]));
        assert_eq!(
            parse(&args(&["send", "{}"])).unwrap(),
            Invocation::Send(vec!["{}".into()])
        );
        assert!(parse(&args(&["nope"])).is_err());
        assert!(parse(&args(&["toggle", "x"])).is_err());
    }

    #[test]
    fn reads_message_lines() {
        let input = "{\"type\":\"command\",\"command\":\"capture\"}\n\n";
        let msgs = read_messages(input.as_bytes()).unwrap();
        assert_eq!(
            msgs,
            vec![ClientMessage::Command {
                command: Command::Capture
            }]
        );
        assert!(read_messages("not json\n".as_bytes()).is_err());
    }
}
