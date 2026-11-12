//! Turning the focused pane into `context` messages for Hitch, and getting
//! them there.
//!
//! Zellij plugins can't open the Hitch socket, so each report runs
//! `hitch send <hello> <context>` as a host process. Every report carries its
//! own `hello`, so Hitch learns on each connection that this session has a
//! plugin that can type commands (the `write_chars` capability), even after
//! Hitch restarts.
//!
//! One `hitch send` runs at a time. If Hitch isn't running the report is
//! retried with backoff (1 s doubling to 30 s, as docs/PROTOCOL.md asks of
//! long-lived clients), and only the newest report is sent.

use std::path::PathBuf;

use hitch_protocol::{ClientMessage, ContextReport, PROTOCOL_VERSION, Source};

pub const CLIENT_NAME: &str = "hitch-zellij";
pub const CAPABILITY_WRITE_CHARS: &str = "write_chars";

const FIRST_RETRY_SECS: f64 = 1.0;
const MAX_RETRY_SECS: f64 = 30.0;

/// What the plugin knows about the focused pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub pane: u32,
    pub cwd: PathBuf,
    pub session: Option<String>,
}

impl Report {
    /// The JSON messages for one `hitch send`: `hello`, then `context`.
    pub fn messages(&self) -> Vec<String> {
        let hello = ClientMessage::Hello {
            client: CLIENT_NAME.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            protocol: PROTOCOL_VERSION,
            capabilities: vec![CAPABILITY_WRITE_CHARS.into()],
        };
        let context = ClientMessage::Context(ContextReport {
            source: Source::Zellij,
            cwd: Some(self.cwd.clone()),
            session: self.session.clone().filter(|s| !s.is_empty()),
            pane: Some(self.pane.to_string()),
            ..Default::default()
        });
        [hello, context]
            .iter()
            .map(|m| serde_json::to_string(m).expect("protocol messages serialize"))
            .collect()
    }
}

/// The `blur` message, sent when the session is closing.
pub fn blur_message() -> String {
    serde_json::to_string(&ClientMessage::Blur {
        source: Source::Zellij,
    })
    .expect("protocol messages serialize")
}

/// What the plugin should do next.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Run `hitch send` with these messages.
    Send(Vec<String>),
    /// Hitch couldn't be reached; call [`Reporter::on_timer`] after this many
    /// seconds.
    RetryIn(f64),
    Idle,
}

#[derive(Debug)]
pub struct Reporter {
    /// The newest report, the one Hitch should end up with.
    desired: Option<Report>,
    /// The report Hitch last accepted.
    sent: Option<Report>,
    /// The report a running `hitch send` carries.
    in_flight: Option<Report>,
    /// A retry timer is set.
    retry_pending: bool,
    next_retry: f64,
}

impl Default for Reporter {
    fn default() -> Self {
        Reporter {
            desired: None,
            sent: None,
            in_flight: None,
            retry_pending: false,
            next_retry: FIRST_RETRY_SECS,
        }
    }
}

impl Reporter {
    /// The focused pane or its folder changed.
    pub fn want(&mut self, report: Report) -> Step {
        self.desired = Some(report);
        self.send_if_needed()
    }

    /// The running `hitch send` finished.
    pub fn on_result(&mut self, ok: bool) -> Step {
        let finished = self.in_flight.take();
        if ok {
            self.sent = finished;
            self.next_retry = FIRST_RETRY_SECS;
            return self.send_if_needed();
        }
        if self.retry_pending {
            return Step::Idle;
        }
        self.retry_pending = true;
        let delay = self.next_retry;
        self.next_retry = (self.next_retry * 2.0).min(MAX_RETRY_SECS);
        Step::RetryIn(delay)
    }

    /// The retry timer fired.
    pub fn on_timer(&mut self) -> Step {
        if !self.retry_pending {
            return Step::Idle;
        }
        self.retry_pending = false;
        self.send_if_needed()
    }

    fn send_if_needed(&mut self) -> Step {
        if self.in_flight.is_some() {
            return Step::Idle;
        }
        match &self.desired {
            Some(d) if self.sent.as_ref() != Some(d) => {
                self.in_flight = Some(d.clone());
                Step::Send(d.messages())
            }
            _ => Step::Idle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hitch_protocol::frame;

    fn report(pane: u32, cwd: &str) -> Report {
        Report {
            pane,
            cwd: cwd.into(),
            session: Some("hitch".into()),
        }
    }

    #[test]
    fn messages_match_the_protocol_doc() {
        let msgs = report(3, "/home/u/code/hitch").messages();
        assert_eq!(
            msgs[0],
            format!(
                r#"{{"type":"hello","client":"hitch-zellij","version":"{}","protocol":1,"capabilities":["write_chars"]}}"#,
                env!("CARGO_PKG_VERSION")
            )
        );
        assert_eq!(
            msgs[1],
            r#"{"type":"context","source":"zellij","cwd":"/home/u/code/hitch","session":"hitch","pane":"3"}"#
        );
        // And Hitch's own decoder (what `hitch send` uses) accepts them.
        for m in &msgs {
            assert!(!matches!(
                frame::decode::<ClientMessage>(m).unwrap(),
                ClientMessage::Unknown
            ));
        }
        assert_eq!(blur_message(), r#"{"type":"blur","source":"zellij"}"#);
    }

    #[test]
    fn empty_session_is_omitted() {
        let mut r = report(1, "/tmp");
        r.session = Some(String::new());
        assert!(!r.messages()[1].contains("session"));
    }

    #[test]
    fn sends_once_and_skips_duplicates() {
        let mut rep = Reporter::default();
        assert!(matches!(rep.want(report(1, "/a")), Step::Send(_)));
        assert_eq!(rep.on_result(true), Step::Idle);
        assert_eq!(rep.want(report(1, "/a")), Step::Idle);
        assert!(matches!(rep.want(report(1, "/b")), Step::Send(_)));
    }

    #[test]
    fn only_newest_report_follows_a_running_send() {
        let mut rep = Reporter::default();
        assert!(matches!(rep.want(report(1, "/a")), Step::Send(_)));
        assert_eq!(rep.want(report(2, "/b")), Step::Idle);
        assert_eq!(rep.want(report(3, "/c")), Step::Idle);
        match rep.on_result(true) {
            Step::Send(m) => assert!(m[1].contains("/c")),
            other => panic!("expected a send, got {other:?}"),
        }
        assert_eq!(rep.on_result(true), Step::Idle);
    }

    #[test]
    fn retries_with_backoff_until_hitch_answers() {
        let mut rep = Reporter::default();
        rep.want(report(1, "/a"));
        let mut delays = vec![];
        for _ in 0..7 {
            match rep.on_result(false) {
                Step::RetryIn(d) => delays.push(d),
                other => panic!("expected a retry, got {other:?}"),
            }
            assert!(matches!(rep.on_timer(), Step::Send(_)));
        }
        assert_eq!(delays, [1.0, 2.0, 4.0, 8.0, 16.0, 30.0, 30.0]);
        assert_eq!(rep.on_result(true), Step::Idle);
        // A later failure starts again from 1 s.
        rep.want(report(1, "/b"));
        assert_eq!(rep.on_result(false), Step::RetryIn(1.0));
    }

    #[test]
    fn timer_without_retry_does_nothing() {
        let mut rep = Reporter::default();
        assert_eq!(rep.on_timer(), Step::Idle);
    }
}
