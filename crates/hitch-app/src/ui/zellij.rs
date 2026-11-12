//! Sending a code block to Zellij (spec F13).
//!
//! Hitch types the text into the last focused pane of the Zellij session
//! that last reported to it, and leaves it unrun unless asked to run it.
//! Three routes, tried in order (docs/PROTOCOL.md):
//!
//! 1. A plugin connected to the socket right now gets a `write_chars`
//!    message ([`super::backend::Engine::deliver`]).
//! 2. A session whose plugin said `hello` gets the same message through
//!    `zellij --session <s> pipe --name hitch`, with a timeout, and counts
//!    as typed only if the plugin printed its ack: with no plugin
//!    listening, `zellij pipe` exits 0 at once (stdin closed) or never
//!    (stdin open).
//! 3. Otherwise, or if the pipe fails, `zellij --session <s> action
//!    write-chars` types the same keystrokes into the session's focused
//!    pane.
//!
//! Routes 2 and 3 run `zellij` and wait for it, so call [`send`] off the UI
//! thread.

use std::ffi::OsStr;
use std::io::{self, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use hitch_protocol::{ServerMessage, ZELLIJ_PIPE_ACK, frame};

/// How long to wait for `zellij pipe`. The plugin answers at once, so a
/// longer wait means it isn't listening.
const PIPE_TIMEOUT: Duration = Duration::from_secs(2);
/// How long to wait for any other `zellij` command.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

/// The Zellij session to send to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub session: String,
    /// The session's plugin said `hello` with `write_chars`.
    pub has_plugin: bool,
}

/// How the text got there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Plugin,
    WriteChars,
}

/// The `zellij` program to run. Tests point it at a stand-in.
#[derive(Debug, Clone)]
pub struct Zellij {
    program: std::path::PathBuf,
    /// Arguments before zellij's own (the stand-in script, in tests).
    #[cfg(test)]
    pre_args: Vec<std::path::PathBuf>,
}

impl Default for Zellij {
    fn default() -> Self {
        Self {
            program: "zellij".into(),
            #[cfg(test)]
            pre_args: Vec::new(),
        }
    }
}

impl Zellij {
    #[cfg(test)]
    fn at(program: impl Into<std::path::PathBuf>) -> Self {
        Self {
            program: program.into(),
            pre_args: Vec::new(),
        }
    }

    /// Runs `script` with `sh`. Executing a script the test just wrote can
    /// fail with ETXTBSY while another test thread's fork still holds it
    /// open for writing; `sh` only reads it.
    #[cfg(test)]
    fn script(script: impl Into<std::path::PathBuf>) -> Self {
        Self {
            program: "/bin/sh".into(),
            pre_args: vec![script.into()],
        }
    }

    /// Types `text` into the target session's last focused pane; with
    /// `submit`, also presses Enter. Blocks until `zellij` is done.
    pub fn send(&self, target: &Target, text: &str, submit: bool) -> Result<Route, String> {
        let sessions = self.sessions().map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => "Can't send to Zellij: zellij isn't on PATH.".to_string(),
            _ => format!("Can't send to Zellij: {e}"),
        })?;
        // `zellij --session <unknown> pipe` exits 0 after listing sessions,
        // so check first.
        if !sessions.contains(&target.session) {
            return Err(format!(
                "The Zellij session \"{}\" isn't running.",
                target.session
            ));
        }
        if target.has_plugin {
            let msg = ServerMessage::WriteChars {
                text: text.to_string(),
                submit,
            };
            let json = frame::encode(&msg).map_err(|e| e.to_string())?;
            if self.pipe(&target.session, json.trim_end()).is_ok() {
                return Ok(Route::Plugin);
            }
            // The plugin couldn't type it (no terminal pane focused yet, or
            // unloaded since it said hello): fall back.
        }
        self.write_chars(&target.session, text, submit)?;
        Ok(Route::WriteChars)
    }

    /// Names of the running sessions.
    fn sessions(&self) -> io::Result<Vec<String>> {
        let out = self.run(
            ["list-sessions", "--short", "--no-formatting"],
            COMMAND_TIMEOUT,
        )?;
        // With no sessions, zellij says so and exits 1.
        if !out.status.success() {
            return Ok(Vec::new());
        }
        Ok(out
            .stdout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect())
    }

    fn pipe(&self, session: &str, json: &str) -> Result<(), String> {
        let out = self
            .run(
                ["--session", session, "pipe", "--name", "hitch", "--", json],
                PIPE_TIMEOUT,
            )
            .map_err(|e| e.to_string())?;
        // Exit 0 without the plugin's ack means nothing was listening.
        if out.status.success() && out.stdout.lines().any(|l| l.trim() == ZELLIJ_PIPE_ACK) {
            Ok(())
        } else {
            Err(out.stdout.trim().to_string())
        }
    }

    fn write_chars(&self, session: &str, text: &str, submit: bool) -> Result<(), String> {
        let keys = hitch_protocol::keys::keystrokes(text, submit);
        if keys.is_empty() {
            return Ok(());
        }
        let out = self
            .run(
                [
                    "--session",
                    session,
                    "action",
                    "write-chars",
                    "--",
                    keys.as_str(),
                ],
                COMMAND_TIMEOUT,
            )
            .map_err(|e| format!("Can't send to Zellij: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            let reason = out.stdout.lines().next().unwrap_or("").trim().to_string();
            Err(format!("Zellij couldn't type it: {reason}"))
        }
    }

    /// Runs `zellij <args>`, killing it after `timeout`.
    fn run<'a>(
        &self,
        args: impl IntoIterator<Item = &'a str>,
        timeout: Duration,
    ) -> io::Result<Output> {
        let mut command = Command::new(&self.program);
        #[cfg(test)]
        command.args(&self.pre_args);
        let mut child = command
            .args(args.into_iter().map(OsStr::new))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "zellij didn't answer in time",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        // Zellij's answers are a few lines, well inside the pipe buffer, so
        // reading after it exits can't deadlock.
        let mut stdout = String::new();
        if let Some(mut out) = child.stdout.take() {
            let _ = out.read_to_string(&mut stdout);
        }
        Ok(Output { status, stdout })
    }
}

struct Output {
    status: ExitStatus,
    stdout: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    /// A stand-in `zellij` that logs its arguments (each ended by 0x1F, each
    /// run by 0x1E) and acts as `behaviour` says.
    struct Fake {
        dir: PathBuf,
    }

    impl Fake {
        fn new(tag: &str, behaviour: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("hitch-fake-zellij-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let script = format!(
                "#!/bin/sh\n\
                 for a in \"$@\"; do printf '%s\\037' \"$a\" >> '{log}'; done\n\
                 printf '\\036' >> '{log}'\n\
                 case \"$*\" in\n\
                 \"list-sessions --short --no-formatting\") printf 'other\\nmain\\n' ;;\n\
                 {behaviour}\n\
                 esac\n",
                log = dir.join("log").display(),
            );
            let bin = dir.join("zellij");
            std::fs::write(&bin, script).unwrap();
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self { dir }
        }

        fn zellij(&self) -> Zellij {
            Zellij::script(self.dir.join("zellij"))
        }

        /// The argument lists of each run, after `list-sessions`.
        fn calls(&self) -> Vec<Vec<String>> {
            let log = std::fs::read_to_string(self.dir.join("log")).unwrap_or_default();
            log.split('\x1e')
                .filter(|c| !c.is_empty())
                .map(|c| {
                    c.trim_end_matches('\x1f')
                        .split('\x1f')
                        .map(String::from)
                        .collect::<Vec<_>>()
                })
                .filter(|c| c[0] != "list-sessions")
                .collect()
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn target(session: &str, has_plugin: bool) -> Target {
        Target {
            session: session.into(),
            has_plugin,
        }
    }

    fn write_chars(session: &str, keys: &str) -> Vec<String> {
        ["--session", session, "action", "write-chars", "--", keys]
            .map(String::from)
            .to_vec()
    }

    #[test]
    fn without_the_plugin_types_with_write_chars() {
        let fake = Fake::new("plain", "");
        let route = fake
            .zellij()
            .send(&target("main", false), "cargo run\n", false)
            .unwrap();
        assert_eq!(route, Route::WriteChars);
        assert_eq!(fake.calls(), vec![write_chars("main", "cargo run")]);
    }

    #[test]
    fn shift_click_presses_enter_and_lines_are_pasted() {
        let fake = Fake::new("submit", "");
        let z = fake.zellij();
        z.send(&target("main", false), "-n x", true).unwrap();
        z.send(&target("main", false), "cd a\nmake\n", false)
            .unwrap();
        assert_eq!(
            fake.calls(),
            vec![
                write_chars("main", "-n x\r"),
                write_chars("main", "\x1b[200~cd a\nmake\x1b[201~"),
            ]
        );
    }

    #[test]
    fn with_the_plugin_pipes_the_message() {
        let fake = Fake::new("pipe", "*pipe*) echo typed ;;");
        let route = fake
            .zellij()
            .send(&target("main", true), "ls", true)
            .unwrap();
        assert_eq!(route, Route::Plugin);
        assert_eq!(
            fake.calls(),
            vec![
                [
                    "--session",
                    "main",
                    "pipe",
                    "--name",
                    "hitch",
                    "--",
                    r#"{"type":"write_chars","text":"ls","submit":true}"#,
                ]
                .map(String::from)
                .to_vec()
            ]
        );
    }

    #[test]
    fn a_failed_or_hung_pipe_falls_back() {
        // The plugin says no terminal pane has had focus yet.
        let fake = Fake::new("pipe-fails", "*pipe*) echo 'no pane'; exit 1 ;;");
        let route = fake
            .zellij()
            .send(&target("main", true), "ls", false)
            .unwrap();
        assert_eq!(route, Route::WriteChars);
        assert_eq!(fake.calls().last(), Some(&write_chars("main", "ls")));

        // No plugin listening: zellij pipe exits 0 and says nothing...
        let fake = Fake::new("pipe-silent", "");
        let route = fake
            .zellij()
            .send(&target("main", true), "ls", false)
            .unwrap();
        assert_eq!(route, Route::WriteChars);
        assert_eq!(fake.calls().last(), Some(&write_chars("main", "ls")));

        // ...or, with its stdin open, waits forever.
        let fake = Fake::new("pipe-hangs", "*pipe*) sleep 30 ;;");
        let started = Instant::now();
        let route = fake
            .zellij()
            .send(&target("main", true), "ls", false)
            .unwrap();
        assert_eq!(route, Route::WriteChars);
        assert!(started.elapsed() < PIPE_TIMEOUT + Duration::from_secs(2));
        assert_eq!(fake.calls().last(), Some(&write_chars("main", "ls")));
    }

    #[test]
    fn a_gone_session_or_missing_zellij_is_an_error() {
        let fake = Fake::new("gone", "");
        let err = fake
            .zellij()
            .send(&target("old", true), "ls", false)
            .unwrap_err();
        assert!(err.contains("\"old\" isn't running"), "{err}");
        assert!(fake.calls().is_empty());

        let err = Zellij::at(Path::new("/nonexistent/zellij"))
            .send(&target("main", false), "ls", false)
            .unwrap_err();
        assert!(err.contains("isn't on PATH"), "{err}");
    }

    #[test]
    fn write_chars_errors_are_reported() {
        let fake = Fake::new("wc-fails", "*write-chars*) echo 'boom'; exit 1 ;;");
        let err = fake
            .zellij()
            .send(&target("main", false), "ls", false)
            .unwrap_err();
        assert!(err.contains("boom"), "{err}");
    }
}
