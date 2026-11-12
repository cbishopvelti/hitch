//! Runs the watcher for real on the macOS CI runner: it needs the main
//! thread and its run loop, so this test has no harness. Elsewhere it does
//! nothing.
//!
//! What a CI runner can show: the Accessibility state is reported, an app
//! switch (opening TextEdit) is seen through NSWorkspace, and pinning with
//! no windows, shortcuts and teardown don't crash. Window titles need the
//! Accessibility permission, so those are only checked when it is there
//! (GitHub's macOS runners have it).

#[cfg(target_os = "macos")]
fn main() {
    use std::process::Command;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use hitch_macos::{FocusEvent, FocusWatcher, Hotkeys};
    use objc2_core_foundation::{CFRunLoop, kCFRunLoopDefaultMode};

    /// Spins the main run loop (which drains the main dispatch queue) until
    /// `done` or the timeout.
    fn pump(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
        let end = Instant::now() + timeout;
        while Instant::now() < end {
            // SAFETY: reading CoreFoundation's constant mode name.
            let mode = unsafe { kCFRunLoopDefaultMode };
            CFRunLoop::run_in_mode(mode, 0.1, false);
            if done() {
                return true;
            }
        }
        false
    }

    let (tx, rx) = mpsc::channel();
    // Started from another thread, as the engine may be: the work hops to
    // the main queue.
    let watcher = std::thread::spawn(move || FocusWatcher::start(move |e| tx.send(e).unwrap()))
        .join()
        .unwrap();

    let mut events = Vec::new();
    let drain = |events: &mut Vec<FocusEvent>| events.extend(rx.try_iter());

    assert!(
        pump(Duration::from_secs(5), || {
            drain(&mut events);
            events
                .iter()
                .any(|e| matches!(e, FocusEvent::Accessibility(_)))
        }),
        "no Accessibility state reported: {events:?}"
    );
    let trusted = events.contains(&FocusEvent::Accessibility(true));
    println!("accessibility trusted: {trusted}");

    // An app switch, to a window showing a file.
    let file = std::env::temp_dir().join(format!("hitch-on-a-mac-{}.txt", std::process::id()));
    std::fs::write(&file, "Hitch test\n").unwrap();
    let file_name = file.file_name().unwrap().to_str().unwrap().to_owned();
    let opened = Command::new("open")
        .args(["-a", "TextEdit"])
        .arg(&file)
        .status()
        .is_ok_and(|s| s.success());
    assert!(opened, "could not open TextEdit");
    let seen = pump(Duration::from_secs(20), || {
        drain(&mut events);
        events.iter().any(|e| {
            matches!(e, FocusEvent::Focus(f)
                if f.bundle_id == "com.apple.TextEdit" && f.name == "TextEdit" && f.pid > 0)
        })
    });
    // With the permission, the window's title and file come through too.
    let window_seen = trusted
        && pump(Duration::from_secs(20), || {
            drain(&mut events);
            events.iter().any(|e| {
                matches!(e, FocusEvent::Focus(f)
                    if f.bundle_id == "com.apple.TextEdit"
                        && f.title.contains(&file_name)
                        && f.document.as_ref().and_then(|d| d.file_name())
                            == Some(std::ffi::OsStr::new(&file_name)))
            })
        });
    let _ = Command::new("pkill").args(["-x", "TextEdit"]).status();
    let _ = std::fs::remove_file(&file);
    for e in &events {
        if let FocusEvent::Focus(f) = e {
            // Titles are fine to print here: it's a CI runner.
            println!(
                "focus: {} ({}) {:?} {:?}",
                f.bundle_id, f.name, f.title, f.document
            );
        }
    }
    assert!(seen, "TextEdit's activation was not reported");
    if trusted {
        assert!(
            window_seen,
            "TextEdit's window title and file were not reported"
        );
    }

    // Pinning with no windows open, and shortcuts that parse.
    hitch_macos::set_keep_above(true);
    hitch_macos::set_keep_above(false);
    let hotkeys = Hotkeys::register(
        vec![
            ("Cmd+Shift+N".to_owned(), Box::new(|| {})),
            ("Cmd+Shift+H".to_owned(), Box::new(|| {})),
        ],
        |keys| println!("shortcut unavailable: {keys}"),
    )
    .expect("the default shortcuts parse");
    assert!(
        Hotkeys::register(vec![("Cmd+Shift+Nope".to_owned(), Box::new(|| {}))], |_| {}).is_err()
    );
    pump(Duration::from_millis(500), || false);

    drop(hotkeys);
    drop(watcher);
    pump(Duration::from_millis(500), || false);
    println!("ok");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
