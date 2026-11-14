//! `hitch-web`: a small window that shows one local web page.
//!
//! Hitch opens it for clawswap's branch page (`http://127.0.0.1:27390/p/<id>`),
//! so the branch list, its Switch buttons and Pull work beside the panel.
//! iced has no web view, and on Wayland a web view can't be put inside a
//! winit window, so the page gets a window, and a process, of its own:
//! GTK and WebKitGTK (WKWebView on macOS) stay out of the panel.
//!
//! ```text
//! hitch-web [--title <title>] [--pin] <url>
//! ```
//!
//! - Only `http://127.0.0.1` and `http://localhost` pages open here. A link
//!   that leads anywhere else (a pull request on GitHub, say) opens in the
//!   default browser instead.
//! - `--pin` keeps the window above other windows, as a pinned panel is:
//!   the window level on X11 and macOS, the Hitch GNOME extension's
//!   `SetKeepAbove` on Wayland.
//! - The window has Hitch's app id, so Hitch doesn't take it for the app
//!   you're working in when it takes focus.

use std::process::ExitCode;

use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::{NewWindowResponse, WebViewBuilder};

const USAGE: &str = "usage: hitch-web [--title <title>] [--pin] <url>";

#[derive(Debug, PartialEq, Eq)]
struct Args {
    url: String,
    title: String,
    pin: bool,
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut url = None;
    let mut title = None;
    let mut pin = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--pin" => pin = true,
            "--title" => title = Some(args.next().ok_or("--title needs a value")?),
            "-h" | "--help" => return Err(USAGE.into()),
            s if s.starts_with('-') => return Err(format!("unknown option {s}\n{USAGE}")),
            _ if url.is_some() => return Err(USAGE.into()),
            _ => url = Some(arg),
        }
    }
    let url = url.ok_or(USAGE)?;
    if !is_local(&url) {
        return Err(format!(
            "hitch-web only opens http://127.0.0.1 and http://localhost pages, not {url}"
        ));
    }
    Ok(Args {
        title: title.unwrap_or_else(|| "Hitch".into()),
        url,
        pin,
    })
}

/// Whether `url` is an `http` page on this machine's loopback address.
fn is_local(url: &str) -> bool {
    let Some(rest) = strip_prefix_ignore_case(url, "http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = match authority.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host,
        _ => authority,
    };
    host == "127.0.0.1" || host.eq_ignore_ascii_case("localhost")
}

fn strip_prefix_ignore_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.get(..prefix.len())
        .filter(|p| p.eq_ignore_ascii_case(prefix))
        .map(|_| &s[prefix.len()..])
}

/// Opens a page in the default browser. Only web links, as the panel does
/// for links in notes.
fn open_in_browser(url: &str) {
    if strip_prefix_ignore_case(url, "https://").is_none()
        && strip_prefix_ignore_case(url, "http://").is_none()
    {
        return;
    }
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    if let Ok(mut child) = std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || child.wait());
    }
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hitch-web: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    // The Wayland app id and X11 WM_CLASS, which Hitch's focus tracking
    // skips as its own. Set through the program name rather than tao's
    // `with_app_id`, which makes a single-instance GApplication that quits
    // at once when it can't register on the session bus.
    #[cfg(target_os = "linux")]
    gtk::glib::set_prgname(Some(hitch_protocol::APP_ID));
    let event_loop = EventLoopBuilder::new().build();
    let window = WindowBuilder::new()
        .with_title(&args.title)
        .with_inner_size(LogicalSize::new(440.0, 680.0))
        .with_always_on_top(args.pin)
        .build(&event_loop)?;

    let webview = WebViewBuilder::new()
        .with_url(&args.url)
        .with_navigation_handler(|url| {
            if is_local(&url) || url.starts_with("about:") {
                true
            } else {
                open_in_browser(&url);
                false
            }
        })
        .with_new_window_req_handler(|url, _| {
            open_in_browser(&url);
            NewWindowResponse::Deny
        });
    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        webview.build_gtk(window.default_vbox().ok_or("the window has no box")?)?
    };
    #[cfg(not(target_os = "linux"))]
    let webview = webview.build(&window)?;

    // Held for as long as the window is open: the extension drops the pin
    // when this connection leaves the bus.
    #[cfg(target_os = "linux")]
    let _keep_above = args.pin.then(keep_above).flatten();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        let _ = &webview;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    });
}

/// Asks the Hitch GNOME extension to keep this process's windows above
/// others (only GNOME on Wayland needs it). `None` when there's no session
/// bus or no extension; the window level still applies on X11.
#[cfg(target_os = "linux")]
fn keep_above() -> Option<zbus::blocking::Connection> {
    let conn = zbus::blocking::Connection::session().ok()?;
    conn.call_method(
        Some("io.github.cbishopvelti.Hitch.Shell"),
        "/io/github/cbishopvelti/Hitch/Shell",
        Some("io.github.cbishopvelti.Hitch.Shell"),
        "SetKeepAbove",
        &(true,),
    )
    .ok()?;
    Some(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Args, String> {
        parse(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_url_title_and_pin() {
        assert_eq!(
            args(&[
                "--title",
                "hitch · clawswap",
                "--pin",
                "http://127.0.0.1:27390/p/hitch"
            ]),
            Ok(Args {
                url: "http://127.0.0.1:27390/p/hitch".into(),
                title: "hitch · clawswap".into(),
                pin: true,
            })
        );
        assert_eq!(args(&["http://localhost:27390/"]).unwrap().title, "Hitch");
    }

    #[test]
    fn needs_one_url() {
        assert!(args(&[]).is_err());
        assert!(args(&["http://127.0.0.1/", "http://127.0.0.1/"]).is_err());
        assert!(args(&["--title"]).is_err());
        assert!(args(&["--zoom", "http://127.0.0.1/"]).is_err());
    }

    #[test]
    fn only_loopback_pages() {
        for url in [
            "http://127.0.0.1:27390/p/hitch",
            "http://localhost:27390",
            "HTTP://LOCALHOST/x?y#z",
            "http://127.0.0.1",
        ] {
            assert!(is_local(url), "{url}");
        }
        for url in [
            "https://127.0.0.1:27390/",
            "http://127.0.0.1.evil.example/",
            "http://localhost.example:27390/",
            "http://user@evil.example/?localhost",
            "http://example.com/127.0.0.1",
            "file:///etc/passwd",
            "javascript:alert(1)",
        ] {
            assert!(!is_local(url), "{url}");
        }
        assert!(args(&["https://github.com"]).is_err());
    }
}
