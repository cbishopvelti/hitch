//! `hitch-web`: a small window that shows one local web page.
//!
//! Hitch opens it for clawswap's branch page (`http://127.0.0.1:27390/p/<id>`),
//! so the branch list, its Switch buttons and Pull work beside the panel.
//! iced has no web view, and on Wayland a web view can't be put inside a
//! winit window, so the page gets a window, and a process, of its own:
//! GTK and WebKitGTK (WKWebView on macOS) stay out of the panel.
//!
//! ```text
//! hitch-web [options] <url>
//!   --title <text>      the window's title
//!   --heading <text>    draw the window as a part of the panel: no frame,
//!                       and a header with this heading, the title and Close
//!   --pin               keep above other windows
//!   --size <W>x<H>      the window's size
//!   --position <X>,<Y>  where to put it (X11 and macOS; Wayland ignores it)
//!   --dock-to <pid>     keep it beside that process's window (GNOME)
//!   --dark              dark page and header
//!   --bg/--fg/--line <#rrggbb>  the header's colours
//!   --css <text>        a stylesheet added to the page
//! ```
//!
//! - Only `http://127.0.0.1` and `http://localhost` pages open here. A link
//!   that leads anywhere else (a pull request on GitHub, say) opens in the
//!   default browser instead.
//! - `--pin` keeps the window above other windows, as a pinned panel is:
//!   the window level on X11 and macOS, the Hitch GNOME extension's
//!   `SetKeepAbove` on Wayland.
//! - `--dock-to` asks the Hitch GNOME extension (`DockBeside`) to keep the
//!   window against the panel's side at the panel's height, and to move it
//!   when the panel moves. Only the shell can place windows on Wayland.
//! - The window has Hitch's app id, so Hitch doesn't take it for the app
//!   you're working in when it takes focus.

use std::process::ExitCode;

use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::{NewWindowResponse, WebViewBuilder};

const USAGE: &str = "usage: hitch-web [--title <text>] [--heading <text>] [--pin] \
[--size <W>x<H>] [--position <X>,<Y>] [--dock-to <pid>] [--dark] \
[--bg|--fg|--line <#rrggbb>] [--css <text>] <url>";

const DEFAULT_SIZE: (f64, f64) = (380.0, 620.0);

/// The header's colours.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Colors {
    bg: String,
    fg: String,
    line: String,
}

impl Colors {
    fn default_for(dark: bool) -> Self {
        let (bg, fg, line) = if dark {
            ("#2b2d31", "#f2f3f5", "#45484f")
        } else {
            ("#f4f4f4", "#000000", "#d9d9d9")
        };
        Self {
            bg: bg.into(),
            fg: fg.into(),
            line: line.into(),
        }
    }
}

#[derive(Debug, PartialEq)]
struct Args {
    url: String,
    title: String,
    heading: Option<String>,
    pin: bool,
    size: (f64, f64),
    position: Option<(f64, f64)>,
    dock_to: Option<u32>,
    dark: bool,
    colors: Colors,
    css: Option<String>,
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut url = None;
    let mut title = None;
    let mut heading = None;
    let mut pin = false;
    let mut size = DEFAULT_SIZE;
    let mut position = None;
    let mut dock_to = None;
    let mut dark = false;
    let (mut bg, mut fg, mut line) = (None, None, None);
    let mut css = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--pin" => pin = true,
            "--dark" => dark = true,
            "--title" => title = Some(value()?),
            "--heading" => heading = Some(value()?),
            "--css" => css = Some(value()?),
            "--size" => {
                size = pair(&value()?, 'x')
                    .filter(|(w, h)| *w >= 1.0 && *h >= 1.0)
                    .ok_or("--size takes <W>x<H>")?;
            }
            "--position" => {
                position = Some(pair(&value()?, ',').ok_or("--position takes <X>,<Y>")?)
            }
            "--dock-to" => {
                dock_to = Some(
                    value()?
                        .parse()
                        .map_err(|_| "--dock-to takes a process id")?,
                );
            }
            "--bg" => bg = Some(color(&value()?)?),
            "--fg" => fg = Some(color(&value()?)?),
            "--line" => line = Some(color(&value()?)?),
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
    let defaults = Colors::default_for(dark);
    Ok(Args {
        title: title.unwrap_or_else(|| "Hitch".into()),
        heading,
        url,
        pin,
        size,
        position,
        dock_to,
        dark,
        colors: Colors {
            bg: bg.unwrap_or(defaults.bg),
            fg: fg.unwrap_or(defaults.fg),
            line: line.unwrap_or(defaults.line),
        },
        css,
    })
}

/// Two numbers with `separator` between them.
fn pair(value: &str, separator: char) -> Option<(f64, f64)> {
    let (a, b) = value.split_once(separator)?;
    let (a, b): (f64, f64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    (a.is_finite() && b.is_finite()).then_some((a, b))
}

/// A `#rrggbb` colour, and nothing else: it goes into a stylesheet.
fn color(value: &str) -> Result<String, String> {
    let hex = value.strip_prefix('#').unwrap_or_default();
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(value.to_ascii_lowercase())
    } else {
        Err(format!("{value} isn't a #rrggbb colour"))
    }
}

/// The script that adds `css` to each page as it loads.
fn css_script(css: &str) -> String {
    let mut quoted = String::from("\"");
    for c in css.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => {}
            '<' => quoted.push_str("\\u003c"),
            '\u{2028}' => quoted.push_str("\\u2028"),
            '\u{2029}' => quoted.push_str("\\u2029"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    format!(
        "(function(){{var add=function(){{var s=document.createElement('style');\
         s.textContent={quoted};(document.head||document.documentElement).appendChild(s);}};\
         if(document.head)add();else document.addEventListener('DOMContentLoaded',add);}})();"
    )
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

/// Sent from the header's Close button.
#[derive(Debug, Clone, Copy)]
struct Close;

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    // The Wayland app id and X11 WM_CLASS, which Hitch's focus tracking
    // skips as its own. Set through the program name rather than tao's
    // `with_app_id`, which makes a single-instance GApplication that quits
    // at once when it can't register on the session bus.
    #[cfg(target_os = "linux")]
    gtk::glib::set_prgname(Some(hitch_protocol::APP_ID));
    let event_loop = EventLoopBuilder::<Close>::with_user_event().build();
    // The header is drawn with GTK, so only Linux goes frameless; elsewhere
    // the window keeps its title bar.
    let own_header = cfg!(target_os = "linux") && args.heading.is_some();
    let mut window = WindowBuilder::new()
        .with_title(&args.title)
        .with_inner_size(LogicalSize::new(args.size.0, args.size.1))
        .with_decorations(!own_header)
        .with_always_on_top(args.pin);
    if let Some((x, y)) = args.position {
        window = window.with_position(LogicalPosition::new(x, y));
    }
    let window = window.build(&event_loop)?;

    #[cfg(target_os = "linux")]
    linux::style(&args);
    #[cfg(target_os = "linux")]
    if let Some(heading) = &args.heading {
        let proxy = event_loop.create_proxy();
        linux::add_header(&window, heading, &args.title, move || {
            let _ = proxy.send_event(Close);
        });
    }

    let mut webview = WebViewBuilder::new()
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
    if let Some(css) = &args.css {
        webview = webview.with_initialization_script(css_script(css));
    }
    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        webview.build_gtk(window.default_vbox().ok_or("the window has no box")?)?
    };
    #[cfg(not(target_os = "linux"))]
    let webview = webview.build(&window)?;

    // Held for as long as the window is open: the extension drops the pin
    // and the dock when this connection leaves the bus.
    #[cfg(target_os = "linux")]
    let _shell = linux::ask_shell(args.pin, args.dock_to);

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        let _ = (&webview, &window);
        match event {
            Event::UserEvent(Close)
            | Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            _ => {}
        }
    });
}

#[cfg(target_os = "linux")]
mod linux {
    use gtk::prelude::*;
    use tao::platform::unix::WindowExtUnix;

    use super::Args;

    const SHELL: &str = "io.github.cbishopvelti.Hitch.Shell";
    const SHELL_PATH: &str = "/io/github/cbishopvelti/Hitch/Shell";

    /// The header's look, after the panel's own: its background, a bold
    /// heading, small flat buttons and a rule underneath. The window's
    /// background shows as a one pixel border around the frameless window.
    pub fn style(args: &Args) {
        if let Some(settings) = gtk::Settings::default() {
            // Also what the page's `prefers-color-scheme` follows.
            settings.set_gtk_application_prefer_dark_theme(args.dark);
        }
        if args.heading.is_none() {
            return;
        }
        let c = &args.colors;
        let css = format!(
            "window {{ background: {line}; }}
             #hitch-header {{ background: {bg}; color: {fg}; border-bottom: 1px solid {line}; }}
             #hitch-header #heading {{ font-weight: bold; font-size: 15px; }}
             #hitch-header #title {{ font-size: 11px; opacity: 0.55; }}
             #hitch-header button {{ font-size: 12px; color: {fg}; background: none;
               border: none; box-shadow: none; padding: 4px 8px; min-height: 0; min-width: 0; }}
             #hitch-header button:hover {{ background: alpha({fg}, 0.08); }}",
            bg = c.bg,
            fg = c.fg,
            line = c.line,
        );
        let provider = gtk::CssProvider::new();
        if provider.load_from_data(css.as_bytes()).is_ok()
            && let Some(screen) = gtk::gdk::Screen::default()
        {
            gtk::StyleContext::add_provider_for_screen(
                &screen,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    }

    /// Puts the header above the page. Dragging it moves the window.
    pub fn add_header(
        window: &tao::window::Window,
        heading: &str,
        title: &str,
        close: impl Fn() + 'static,
    ) {
        let Some(vbox) = window.default_vbox() else {
            return;
        };
        vbox.set_border_width(1);

        let heading = gtk::Label::new(Some(heading));
        heading.set_widget_name("heading");
        let title = gtk::Label::new(Some(title));
        title.set_widget_name("title");
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let button = gtk::Button::with_label("Close");
        button.set_can_focus(false);
        button.connect_clicked(move |_| close());

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_margin_start(12);
        row.set_margin_end(6);
        // With the window's border and the rule, as tall as the panel's header.
        row.set_size_request(-1, 40);
        row.pack_start(&heading, false, false, 0);
        row.pack_start(&title, true, true, 0);
        row.pack_end(&button, false, false, 0);

        let header = gtk::EventBox::new();
        header.set_widget_name("hitch-header");
        header.add(&row);
        let gtk_window = window.gtk_window().clone();
        header.connect_button_press_event(move |_, event| {
            if event.button() == 1 {
                let (x, y) = event.root();
                gtk_window.begin_move_drag(1, x as i32, y as i32, event.time());
            }
            gtk::glib::Propagation::Stop
        });
        vbox.pack_start(&header, false, false, 0);
        header.show_all();
    }

    /// Asks the Hitch GNOME extension to keep this process's windows above
    /// others, and beside `dock_to`'s window (only GNOME on Wayland needs
    /// either). `None` when there's nothing to ask or no session bus; a
    /// missing extension just means the calls fail.
    pub fn ask_shell(pin: bool, dock_to: Option<u32>) -> Option<zbus::blocking::Connection> {
        if !pin && dock_to.is_none() {
            return None;
        }
        let conn = zbus::blocking::Connection::session().ok()?;
        if pin {
            let _ = conn.call_method(
                Some(SHELL),
                SHELL_PATH,
                Some(SHELL),
                "SetKeepAbove",
                &(true,),
            );
        }
        if let Some(pid) = dock_to {
            let _ = conn.call_method(Some(SHELL), SHELL_PATH, Some(SHELL), "DockBeside", &(pid,));
        }
        Some(conn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Args, String> {
        parse(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_url_title_and_pin() {
        let a = args(&[
            "--title",
            "hitch · clawswap",
            "--pin",
            "http://127.0.0.1:27390/p/hitch",
        ])
        .unwrap();
        assert_eq!(a.url, "http://127.0.0.1:27390/p/hitch");
        assert_eq!(a.title, "hitch · clawswap");
        assert!(a.pin);
        assert_eq!(a.size, DEFAULT_SIZE);
        assert_eq!(
            (a.heading, a.position, a.dock_to, a.css),
            (None, None, None, None)
        );
        assert_eq!(args(&["http://localhost:27390/"]).unwrap().title, "Hitch");
    }

    #[test]
    fn parses_the_panel_look() {
        let a = args(&[
            "--heading",
            "Branches",
            "--size",
            "380x600",
            "--position",
            "-20,48.5",
            "--dock-to",
            "4242",
            "--dark",
            "--bg",
            "#2B2D31",
            "--css",
            ":root{--bg:#000}",
            "http://127.0.0.1:27390/p/hitch",
        ])
        .unwrap();
        assert_eq!(a.heading.as_deref(), Some("Branches"));
        assert_eq!(a.size, (380.0, 600.0));
        assert_eq!(a.position, Some((-20.0, 48.5)));
        assert_eq!(a.dock_to, Some(4242));
        assert!(a.dark);
        assert_eq!(a.colors.bg, "#2b2d31");
        // Colours not given follow --dark.
        assert_eq!(a.colors.fg, Colors::default_for(true).fg);
        assert_eq!(a.css.as_deref(), Some(":root{--bg:#000}"));
    }

    #[test]
    fn needs_one_url() {
        assert!(args(&[]).is_err());
        assert!(args(&["http://127.0.0.1/", "http://127.0.0.1/"]).is_err());
        assert!(args(&["--title"]).is_err());
        assert!(args(&["--zoom", "http://127.0.0.1/"]).is_err());
    }

    #[test]
    fn rejects_bad_values() {
        let url = "http://127.0.0.1/";
        assert!(args(&["--size", "380", url]).is_err());
        assert!(args(&["--size", "0x600", url]).is_err());
        assert!(args(&["--position", "left", url]).is_err());
        assert!(args(&["--dock-to", "hitch", url]).is_err());
        // A colour goes into a stylesheet, so nothing but hex digits.
        assert!(args(&["--bg", "red; } * { color: blue", url]).is_err());
        assert!(args(&["--fg", "#12345", url]).is_err());
    }

    #[test]
    fn css_is_quoted_for_the_script() {
        let script = css_script("a::after{content:\"</style>\\\"}\n");
        assert!(
            script.contains(r#"s.textContent="a::after{content:\"\u003c/style>\\\"}\n";"#),
            "{script}"
        );
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
