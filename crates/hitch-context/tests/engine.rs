//! End-to-end tests: a real socket, and on Linux a private D-Bus daemon with
//! a stand-in for the GNOME Shell extension.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use hitch_context::{Config, Context, Engine, Event};
use hitch_protocol::{Command, ServerMessage};

const WAIT: Duration = Duration::from_secs(5);

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("hitch-engine-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn dir(&self, rel: &str) -> PathBuf {
        let p = self.0.join(rel);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn start(sock: &Path, dbus: Option<String>) -> (Engine, mpsc::Receiver<Event>) {
    let (tx, rx) = mpsc::channel();
    let engine = Engine::start(
        Config {
            socket_path: Some(sock.to_path_buf()),
            debounce: Duration::from_millis(20),
            gnome_shell: dbus.is_some(),
            dbus_address: dbus,
            // No prompts or global shortcuts from a test run on macOS.
            macos_focus: false,
            request_accessibility: false,
            global_shortcuts: Vec::new(),
            ..Default::default()
        },
        move |e| {
            let _ = tx.send(e);
        },
    )
    .unwrap();
    (engine, rx)
}

/// The next event that `pick` accepts, skipping others.
fn next<T>(rx: &mpsc::Receiver<Event>, pick: impl Fn(Event) -> Option<T>) -> T {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let e = rx.recv_timeout(left).expect("timed out waiting for event");
        if let Some(t) = pick(e) {
            return t;
        }
    }
}

fn contexts(rx: &mpsc::Receiver<Event>) -> Vec<Context> {
    next(rx, |e| match e {
        Event::Contexts(c) => Some(c.contexts),
        _ => None,
    })
}

struct Client {
    w: UnixStream,
    r: BufReader<UnixStream>,
}

impl Client {
    fn connect(sock: &Path) -> Self {
        let w = UnixStream::connect(sock).unwrap();
        w.set_read_timeout(Some(WAIT)).unwrap();
        let r = BufReader::new(w.try_clone().unwrap());
        Self { w, r }
    }
    fn send(&mut self, line: &str) {
        self.w.write_all(line.as_bytes()).unwrap();
        self.w.write_all(b"\n").unwrap();
    }
    fn recv(&mut self) -> ServerMessage {
        let mut line = String::new();
        self.r.read_line(&mut line).unwrap();
        hitch_protocol::frame::decode(&line).unwrap()
    }
}

#[test]
fn socket_round_trip() {
    let t = TempDir::new("socket");
    let sock = t.0.join("run/hitch.sock");
    let proj = t.dir("proj");
    t.dir("proj/.git");
    let (engine, rx) = start(&sock, None);

    let mode = std::fs::metadata(&sock).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let dir_mode = std::fs::metadata(sock.parent().unwrap())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(dir_mode & 0o777, 0o700);

    // A second Hitch on the same socket is refused.
    let err = Engine::start(
        Config {
            socket_path: Some(sock.clone()),
            gnome_shell: false,
            macos_focus: false,
            request_accessibility: false,
            global_shortcuts: Vec::new(),
            ..Default::default()
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);

    let mut zellij = Client::connect(&sock);
    zellij.send(
        r#"{"type":"hello","client":"hitch-zellij","version":"0","protocol":1,"capabilities":["write_chars"]}"#,
    );
    assert!(matches!(
        zellij.recv(),
        ServerMessage::Welcome { protocol: 1, .. }
    ));

    zellij.send(&format!(
        r#"{{"type":"context","source":"zellij","cwd":"{}","session":"main"}}"#,
        proj.join("src").display()
    ));
    t.dir("proj/src");
    assert_eq!(contexts(&rx), vec![Context::Folder { path: proj.clone() }]);
    assert_eq!(
        engine.current().contexts,
        vec![Context::Folder { path: proj.clone() }]
    );
    assert_eq!(
        engine
            .last_report(&hitch_protocol::Source::Zellij)
            .and_then(|r| r.session),
        Some("main".into())
    );

    // Garbage gets an error, and the connection stays usable.
    zellij.send("not json");
    assert!(matches!(zellij.recv(), ServerMessage::Error { .. }));
    zellij.send(r#"{"type":"from_the_future"}"#);
    zellij.send(r#"{"type":"hello","client":"x","version":"0","protocol":9}"#);
    assert_eq!(
        zellij.recv(),
        ServerMessage::Error {
            message: "unsupported protocol 9".into()
        }
    );

    // Hitch to the plugin.
    let msg = ServerMessage::WriteChars {
        text: "cargo run".into(),
        submit: false,
    };
    assert!(engine.deliver("write_chars", &msg));
    assert_eq!(zellij.recv(), msg);
    assert!(!engine.deliver("something_else", &msg));

    // A one-shot CLI client.
    hitch_protocol::client::send_all(
        &sock,
        &[hitch_protocol::ClientMessage::Command {
            command: Command::TogglePanel,
        }],
    )
    .unwrap();
    assert_eq!(
        next(&rx, |e| match e {
            Event::Command(c) => Some(c),
            _ => None,
        }),
        Command::TogglePanel
    );

    // The plugin going away is a blur; with no other report, its notes stay.
    drop(zellij);
    std::thread::sleep(Duration::from_millis(100));
    assert!(!engine.deliver("write_chars", &msg));
    assert_eq!(
        engine.current().contexts,
        vec![Context::Folder { path: proj }]
    );

    drop(engine);
    std::thread::sleep(Duration::from_millis(50));
    assert!(!sock.exists());
}

#[cfg(target_os = "linux")]
mod dbus {
    use super::*;
    use std::collections::HashMap;
    use std::process::{Child, Stdio};
    use std::sync::{Arc, Mutex};

    use zbus::zvariant::{OwnedValue, Value};

    const BUS: &str = "io.github.cbishopvelti.Hitch.Shell";
    const PATH: &str = "/io/github/cbishopvelti/Hitch/Shell";

    struct Bus {
        child: Child,
        address: String,
    }

    impl Bus {
        fn start() -> Option<Self> {
            let mut child = std::process::Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .ok()?;
            let mut line = String::new();
            BufReader::new(child.stdout.take()?)
                .read_line(&mut line)
                .ok()?;
            let address = line.trim().to_owned();
            (!address.is_empty()).then_some(Self { child, address })
        }
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn focus(app_id: &str, title: &str) -> HashMap<String, OwnedValue> {
        let s = |v: &str| OwnedValue::try_from(Value::from(v)).unwrap();
        HashMap::from([
            ("app_id".into(), s(app_id)),
            ("wm_class".into(), s(app_id)),
            ("title".into(), s(title)),
            ("pid".into(), OwnedValue::from(4242u32)),
            ("sandboxed_app_id".into(), s("")),
        ])
    }

    struct Mock {
        title: String,
        pinned: Arc<Mutex<Vec<bool>>>,
    }

    #[zbus::interface(name = "io.github.cbishopvelti.Hitch.Shell")]
    impl Mock {
        fn get_focus(&self) -> HashMap<String, OwnedValue> {
            focus("dev.zed.Zed", &self.title)
        }

        fn set_keep_above(&self, pinned: bool) {
            self.pinned.lock().unwrap().push(pinned);
        }

        #[zbus(property)]
        fn version(&self) -> u32 {
            1
        }
    }

    #[test]
    fn gnome_shell_extension() {
        let Some(bus) = Bus::start() else {
            eprintln!("dbus-daemon not available, skipping");
            return;
        };
        let t = TempDir::new("dbus");
        t.dir("hitch/.git");
        let file = t.dir("hitch/src").join("main.rs");
        std::fs::write(&file, "").unwrap();
        let other = t.dir("other");
        t.dir("other/.git");
        std::fs::write(other.join("notes.txt"), "").unwrap();

        // Hitch starts before the extension is on the bus.
        let (engine, rx) = start(&t.0.join("hitch.sock"), Some(bus.address.clone()));
        let available = |rx: &mpsc::Receiver<Event>| {
            next(rx, |e| match e {
                Event::ShellExtension { available } => Some(available),
                _ => None,
            })
        };
        assert!(!available(&rx));
        assert_eq!(
            engine.set_keep_above(false).unwrap_err().kind(),
            std::io::ErrorKind::Other
        );

        let pinned = Arc::new(Mutex::new(Vec::new()));
        let ext = zbus::blocking::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .name(BUS)
            .unwrap()
            .serve_at(
                PATH,
                Mock {
                    title: format!("hitch — {}", file.display()),
                    pinned: pinned.clone(),
                },
            )
            .unwrap()
            .build()
            .unwrap();

        // It appears: Hitch asks GetFocus.
        assert!(available(&rx));
        assert_eq!(
            contexts(&rx)[0],
            Context::File {
                project: t.0.join("hitch"),
                file: "src/main.rs".into()
            }
        );

        // FocusChanged to an app with a path in its title.
        ext.emit_signal(
            None::<zbus::names::BusName<'_>>,
            PATH,
            BUS,
            "FocusChanged",
            &(focus(
                "org.gnome.TextEditor",
                &format!("notes.txt ({}) - Text Editor", other.display()),
            ),),
        )
        .unwrap();
        assert_eq!(
            contexts(&rx)[0],
            Context::File {
                project: other.clone(),
                file: "notes.txt".into()
            }
        );

        engine.set_keep_above(true).unwrap();
        assert_eq!(*pinned.lock().unwrap(), vec![true]);

        // The extension leaves the bus (screen lock) and comes back: the
        // pin is applied again.
        drop(ext);
        assert!(!available(&rx));
        let pinned_again = Arc::new(Mutex::new(Vec::new()));
        let _ext = zbus::blocking::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .name(BUS)
            .unwrap()
            .serve_at(
                PATH,
                Mock {
                    title: String::new(),
                    pinned: pinned_again.clone(),
                },
            )
            .unwrap()
            .build()
            .unwrap();
        assert!(available(&rx));
        let deadline = std::time::Instant::now() + WAIT;
        while pinned_again.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(*pinned_again.lock().unwrap(), vec![true]);
    }
}
