//! Hitch plugin for Zellij.
//!
//! - Reports the focused terminal pane's working folder to Hitch, so Hitch
//!   shows the notes of the project that contains it (spec F2).
//! - Types commands Hitch sends (`write_chars`) into the last focused
//!   terminal pane, unrun unless `submit` is set (spec F13).
//!
//! Zellij plugins can't open sockets, so reports go out through
//! `hitch send` and Hitch's messages come in through
//! `zellij pipe --name hitch`. See README.md and docs/PROTOCOL.md.

mod focus;
mod keys;
mod report;

use std::collections::BTreeMap;

use hitch_protocol::{ServerMessage, frame};
use zellij_tile::prelude::*;

use focus::FocusTracker;
use report::{Report, Reporter, Step};

/// Name of the pipe Hitch sends to: `zellij pipe --name hitch -- '<json>'`.
const PIPE_NAME: &str = "hitch";
/// `run_command` context key telling our commands' results apart.
const CTX_KIND: &str = "hitch";
const KIND_REPORT: &str = "report";
const KIND_BLUR: &str = "blur";

#[derive(Default)]
struct HitchPlugin {
    /// The `hitch` executable; plugin option `hitch_path`.
    hitch_path: String,
    granted: bool,
    session: Option<String>,
    focus: FocusTracker,
    reporter: Reporter,
    /// Last thing worth showing if the plugin is opened in a pane.
    status: String,
}

register_plugin!(HitchPlugin);

impl ZellijPlugin for HitchPlugin {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.hitch_path = configuration
            .get("hitch_path")
            .filter(|p| !p.is_empty())
            .cloned()
            .unwrap_or_else(|| "hitch".into());
        self.status = "Waiting for permissions".into();
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::RunCommands,
            PermissionType::WriteToStdin,
            PermissionType::ReadCliPipes,
        ]);
        subscribe(&[
            EventType::PermissionRequestResult,
            EventType::ModeUpdate,
            EventType::SessionUpdate,
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::PaneClosed,
            EventType::CwdChanged,
            EventType::RunCommandResult,
            EventType::Timer,
            EventType::BeforeClose,
        ]);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(PermissionStatus::Granted) => {
                self.granted = true;
                self.status = "Ready".into();
                // The plugin has nothing to interact with; don't let it take
                // focus if it was opened in a pane.
                set_selectable(false);
                self.report_focused();
                true
            }
            Event::PermissionRequestResult(PermissionStatus::Denied) => {
                self.status = "Permissions denied; Hitch can't see this session".into();
                true
            }
            Event::ModeUpdate(mode) => {
                if let Some(name) = mode.session_name {
                    self.set_session(name);
                }
                false
            }
            // Background plugins (loaded with `load_plugins`) may not get a
            // ModeUpdate with the session name; the session list has it.
            Event::SessionUpdate(sessions, _resurrectable) => {
                if let Some(current) = sessions.into_iter().find(|s| s.is_current_session) {
                    self.set_session(current.name);
                }
                false
            }
            Event::TabUpdate(tabs) => {
                if self.focus.on_tabs(&tabs) {
                    self.report_focused();
                }
                false
            }
            Event::PaneUpdate(manifest) => {
                if self.focus.on_panes(manifest) {
                    self.report_focused();
                }
                false
            }
            Event::PaneClosed(PaneId::Terminal(id)) => {
                self.focus.on_pane_closed(id);
                false
            }
            Event::CwdChanged(PaneId::Terminal(id), cwd, _clients) => {
                if self.focus.focused() == Some(id) {
                    self.want(id, cwd);
                }
                false
            }
            Event::RunCommandResult(exit, _stdout, stderr, ctx) => {
                if ctx.get(CTX_KIND).map(String::as_str) == Some(KIND_REPORT) {
                    let ok = exit == Some(0);
                    if ok {
                        self.status = "Connected to Hitch".into();
                    } else {
                        self.status = format!(
                            "Can't reach Hitch ({}): {}",
                            exit.map_or("not run".into(), |c| format!("exit {c}")),
                            String::from_utf8_lossy(&stderr).trim()
                        );
                    }
                    let step = self.reporter.on_result(ok);
                    self.run(step);
                    return true;
                }
                false
            }
            Event::Timer(_) => {
                let step = self.reporter.on_timer();
                self.run(step);
                false
            }
            Event::BeforeClose => {
                if self.granted {
                    self.hitch_send(vec![report::blur_message()], KIND_BLUR);
                }
                false
            }
            _ => false,
        }
    }

    fn pipe(&mut self, msg: PipeMessage) -> bool {
        if msg.name != PIPE_NAME {
            return false;
        }
        let Some(payload) = msg.payload.as_deref() else {
            return false;
        };
        let result = self.handle_server_message(payload);
        if let PipeSource::Cli(pipe_id) = &msg.source {
            // Tell `zellij pipe` (Hitch) whether it worked, so it can fall
            // back to `zellij action write-chars`.
            if let Err(e) = &result {
                cli_pipe_output(pipe_id, &format!("{e}\n"));
                set_cli_pipe_exit_code(pipe_id, 1);
            }
            unblock_cli_pipe_input(pipe_id);
        }
        match result {
            Ok(()) => false,
            Err(e) => {
                self.status = e;
                true
            }
        }
    }

    fn render(&mut self, _rows: usize, _cols: usize) {
        println!("Hitch");
        println!("{}", self.status);
        if let Some(t) = self.focus.target() {
            println!("Commands go to pane {t}");
        }
    }
}

impl HitchPlugin {
    fn handle_server_message(&mut self, payload: &str) -> Result<(), String> {
        let msg: ServerMessage =
            frame::decode(payload).map_err(|e| format!("bad message from Hitch: {e}"))?;
        match msg {
            ServerMessage::WriteChars { text, submit } => {
                if !self.granted {
                    return Err("the Hitch plugin has no permissions yet".into());
                }
                let pane = self
                    .focus
                    .target()
                    .ok_or("no terminal pane has had focus yet")?;
                write_to_pane_id(keys::keystrokes(&text, submit), PaneId::Terminal(pane));
                Ok(())
            }
            ServerMessage::Error { message } => {
                eprintln!("hitch-zellij: Hitch reported an error: {message}");
                Ok(())
            }
            // Welcome only answers a long-lived connection; ignore it and
            // anything newer than this plugin.
            ServerMessage::Welcome { .. } | ServerMessage::Unknown => Ok(()),
        }
    }

    fn set_session(&mut self, name: String) {
        if self.session.as_deref() != Some(name.as_str()) {
            self.session = Some(name);
            self.report_focused();
        }
    }

    /// Look up the focused pane's folder and report it.
    fn report_focused(&mut self) {
        if !self.granted {
            return;
        }
        let Some(id) = self.focus.focused() else {
            return;
        };
        match get_pane_cwd(PaneId::Terminal(id)) {
            Ok(cwd) => self.want(id, cwd),
            Err(e) => eprintln!("hitch-zellij: no cwd for pane {id}: {e}"),
        }
    }

    fn want(&mut self, pane: u32, cwd: std::path::PathBuf) {
        if !self.granted || !cwd.is_absolute() {
            return;
        }
        let step = self.reporter.want(Report {
            pane,
            cwd,
            session: self.session.clone(),
        });
        self.run(step);
    }

    fn run(&mut self, step: Step) {
        match step {
            Step::Send(msgs) => {
                self.hitch_send(msgs, KIND_REPORT);
            }
            Step::RetryIn(secs) => set_timeout(secs),
            Step::Idle => {}
        }
    }

    fn hitch_send(&self, msgs: Vec<String>, kind: &str) {
        let mut cmd: Vec<&str> = vec![&self.hitch_path, "send"];
        cmd.extend(msgs.iter().map(String::as_str));
        let ctx = BTreeMap::from([(CTX_KIND.to_owned(), kind.to_owned())]);
        run_command(&cmd, ctx);
    }
}
