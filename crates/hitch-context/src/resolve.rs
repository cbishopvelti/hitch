//! The resolver: turns window focus and source reports into the contexts
//! that are current (spec, "What a context is").
//!
//! It is plain state with explicit timestamps, so it can be tested without
//! threads or a desktop. The engine feeds it and debounces its output.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use hitch_protocol::zed::{self, ZedTitle};
use hitch_protocol::{Context, ContextReport, Source};

use crate::ActiveContexts;
use crate::project::ProjectFinder;
use crate::title;

/// The focused window, as the GNOME Shell extension (or later macOS
/// detection) reports it. Mirrors the extension's `focus` dictionary in
/// `docs/PROTOCOL.md`; every field is empty or 0 when nothing is focused.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowFocus {
    /// Desktop file id without `.desktop` (macOS: bundle id).
    pub app_id: String,
    /// Wayland app_id or X11 WM_CLASS.
    pub wm_class: String,
    /// Window title. Matched against, never stored beyond the next switch.
    pub title: String,
    /// Process id, 0 if unknown.
    pub pid: u32,
    /// Flatpak or Snap id.
    pub sandboxed_app_id: String,
}

impl WindowFocus {
    /// Nothing focused (desktop, overview).
    pub fn is_empty(&self) -> bool {
        self.app_id.is_empty() && self.wm_class.is_empty() && self.sandboxed_app_id.is_empty()
    }

    /// The ids this window can be matched by, lower case.
    fn ids(&self) -> impl Iterator<Item = String> + '_ {
        [&self.app_id, &self.wm_class, &self.sandboxed_app_id]
            .into_iter()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_lowercase())
    }

    /// The best single name for an [`Context::App`].
    fn app_name(&self) -> &str {
        [&self.app_id, &self.sandboxed_app_id, &self.wm_class]
            .into_iter()
            .find(|s| !s.is_empty())
            .map_or("", |s| s.as_str())
    }

    fn matches_any(&self, patterns: &[String]) -> bool {
        self.ids()
            .any(|id| patterns.iter().any(|p| pattern_matches(p, &id)))
    }
}

/// `pattern` is lower case; a trailing `*` matches any suffix.
fn pattern_matches(pattern: &str, id: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => id.starts_with(prefix),
        None => id == pattern,
    }
}

/// Which apps each plugin source lives in, so its report is used only while
/// that app is frontmost. Patterns are app ids or WM classes, compared
/// without case; a trailing `*` matches any suffix.
///
/// A source can also be learned at run time: when a report arrives while an
/// app with no known source is frontmost, that app is tied to the source
/// (a terminal that isn't in the list, running Zellij).
#[derive(Debug, Clone)]
pub struct AppTable {
    pub entries: Vec<(Source, Vec<String>)>,
}

impl Default for AppTable {
    fn default() -> Self {
        let list = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect();
        Self {
            entries: vec![
                (Source::Zed, list(&["dev.zed.zed*", "zed"])),
                (
                    Source::Zellij,
                    list(&[
                        "org.gnome.terminal*",
                        "gnome-terminal*",
                        "org.gnome.ptyxis*",
                        "ptyxis",
                        "org.gnome.console",
                        "kgx",
                        "com.mitchellh.ghostty",
                        "ghostty",
                        "kitty",
                        "alacritty",
                        "org.alacritty*",
                        "org.wezfurlong.wezterm",
                        "wezterm*",
                        "foot",
                        "footclient",
                        "org.kde.konsole",
                        "konsole",
                        "xterm",
                        "com.raggesilver.blackbox",
                        "com.gexperts.tilix",
                        "tilix",
                        "terminator",
                        "io.elementary.terminal",
                        "dev.warp.warp*",
                        "rio",
                        "com.apple.terminal",
                        "com.googlecode.iterm2",
                    ]),
                ),
                (
                    Source::Clawswap,
                    list(&[
                        "firefox*",
                        "org.mozilla.firefox*",
                        "google-chrome*",
                        "com.google.chrome*",
                        "chromium*",
                        "org.chromium.chromium",
                        "brave-browser*",
                        "com.brave.browser*",
                        "microsoft-edge*",
                        "com.microsoft.edge*",
                        "vivaldi*",
                        "com.vivaldi.vivaldi",
                        "org.gnome.epiphany",
                        "epiphany",
                        "librewolf",
                        "io.gitlab.librewolf-community",
                        "zen*",
                        "app.zen_browser.zen",
                        "com.apple.safari",
                    ]),
                ),
            ],
        }
    }
}

impl AppTable {
    fn sources_for(&self, w: &WindowFocus) -> Vec<&Source> {
        self.entries
            .iter()
            .filter(|(_, pats)| w.matches_any(pats))
            .map(|(s, _)| s)
            .collect()
    }

    fn is_browser(&self, w: &WindowFocus) -> bool {
        self.sources_for(w).contains(&&Source::Clawswap)
    }
}

/// Apps, sites and folders Hitch never records (spec F10). Applied before
/// anything else: an ignored window leaves the panel as it was, and a report
/// inside an ignored folder counts as reporting no file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IgnoreList {
    /// App ids or WM classes; a trailing `*` matches any suffix.
    pub apps: Vec<String>,
    /// Absolute folders; everything inside them is ignored.
    pub folders: Vec<PathBuf>,
    /// Hosts (`example.com` also covers its subdomains) or URL prefixes.
    pub urls: Vec<String>,
}

impl IgnoreList {
    fn window(&self, w: &WindowFocus) -> bool {
        let pats: Vec<String> = self.apps.iter().map(|a| a.to_lowercase()).collect();
        w.matches_any(&pats)
    }

    fn path(&self, p: &Path) -> bool {
        self.folders.iter().any(|f| p.starts_with(f))
    }

    fn url(&self, url: &str) -> bool {
        let host = url_host(url);
        self.urls.iter().any(|entry| {
            url.starts_with(entry.as_str())
                || host.is_some_and(|h| {
                    h.eq_ignore_ascii_case(entry)
                        || h.to_lowercase()
                            .ends_with(&format!(".{}", entry.to_lowercase()))
                })
        })
    }
}

fn url_host(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = match host.rfind(':') {
        Some(i) if !host.ends_with(']') => &host[..i],
        _ => host,
    };
    (!host.is_empty()).then_some(host)
}

/// Window titles of private browser windows, which are always ignored.
const PRIVATE_TITLE_SUFFIXES: &[&str] = &[
    "private browsing",
    "(private)",
    "(incognito)",
    "incognito",
    "private window",
];

#[derive(Debug, Clone)]
struct Report {
    report: ContextReport,
    at: Instant,
    blurred: bool,
}

/// Resolver state. See the module docs.
pub struct Resolver {
    finder: Box<dyn ProjectFinder>,
    apps: AppTable,
    ignore: IgnoreList,
    own_pid: u32,
    home: Option<PathBuf>,
    /// `None` until an OS focus source reports, and again if it goes away.
    window: Option<(WindowFocus, Instant)>,
    reports: HashMap<Source, Report>,
    learned: HashMap<String, Source>,
    /// The last file or folder contexts (matching rule 3).
    sticky: Vec<Context>,
    last: ActiveContexts,
    /// A Zed title had a file name but no path: the user hasn't set Zed's
    /// `window_title_format`.
    zed_needs_setting: bool,
}

impl Resolver {
    pub fn new(finder: Box<dyn ProjectFinder>, apps: AppTable, ignore: IgnoreList) -> Self {
        Self {
            finder,
            apps,
            ignore,
            own_pid: std::process::id(),
            home: std::env::var_os("HOME").map(PathBuf::from),
            window: None,
            reports: HashMap::new(),
            learned: HashMap::new(),
            sticky: Vec::new(),
            last: ActiveContexts::default(),
            zed_needs_setting: false,
        }
    }

    pub fn set_ignore(&mut self, ignore: IgnoreList) {
        self.ignore = ignore;
    }

    #[cfg(test)]
    fn set_home(&mut self, home: &Path) {
        self.home = Some(home.to_path_buf());
    }

    /// A source reported its context.
    pub fn report(&mut self, report: ContextReport, now: Instant) {
        let source = report.source.clone();
        self.reports.insert(
            source,
            Report {
                report,
                at: now,
                blurred: false,
            },
        );
    }

    /// A source lost focus. Its report is no longer preferred; whatever it
    /// showed stays on screen until another file context arrives.
    pub fn blur(&mut self, source: &Source) {
        self.reports.remove(source);
    }

    /// The focused window changed. Returns false if the change was dropped
    /// (Hitch's own panel took focus).
    pub fn window_focus(&mut self, w: WindowFocus, now: Instant) -> bool {
        let own = w.pid != 0 && w.pid == self.own_pid
            || w.ids()
                .any(|id| id.eq_ignore_ascii_case(hitch_protocol::APP_ID));
        if own {
            return false;
        }
        self.window = Some((w, now));
        true
    }

    /// The OS focus source went away (the GNOME extension left the bus).
    /// Plugin reports alone decide until it comes back.
    pub fn clear_window(&mut self) {
        self.window = None;
    }

    /// The contexts current now. Also updates the sticky file context.
    pub fn resolve(&mut self) -> ActiveContexts {
        let out = match self.window.clone() {
            None => {
                let latest = self.latest_report(|_| true).cloned();
                match latest.map(|r| self.contexts_from_report(&r.report)) {
                    Some(ctx) if !ctx.is_empty() => self.set_sticky(ctx),
                    _ => self.sticky.clone(),
                }
            }
            Some((w, _)) if w.is_empty() => self.sticky.clone(),
            Some((w, w_at)) => {
                if self.ignore.window(&w) || self.is_private(&w) {
                    return self.last.clone();
                }
                self.resolve_window(&w, w_at)
            }
        };
        self.last = ActiveContexts::new(out);
        self.last.clone()
    }

    /// True once, after a Zed window showed Zed's default title, which has
    /// no path. The app can then suggest [`zed::RECOMMENDED_SETTINGS`].
    pub fn take_zed_title_hint(&mut self) -> bool {
        std::mem::take(&mut self.zed_needs_setting)
    }

    fn resolve_window(&mut self, w: &WindowFocus, w_at: Instant) -> Vec<Context> {
        // Zed has no extension API for the active file; its window title
        // carries the path instead (docs/PROTOCOL.md, "Zed (window title)").
        if w.ids().any(|id| zed::is_zed_app_id(&id)) {
            match zed::parse_window_title(&w.title) {
                Some(t @ ZedTitle::File { .. }) => {
                    // A path that isn't on this machine is a remote project.
                    if let Some(report) = t.report().filter(|_| t.file().is_some_and(Path::is_file))
                    {
                        let ctx = self.contexts_from_report(&report);
                        if !ctx.is_empty() {
                            return self.set_sticky(ctx);
                        }
                    }
                }
                Some(ZedTitle::FileName { .. }) => self.zed_needs_setting = true,
                _ => {}
            }
        }

        let mut sources: Vec<Source> = self.apps.sources_for(w).into_iter().cloned().collect();
        sources.extend(w.ids().filter_map(|id| self.learned.get(&id).cloned()));

        let chosen = if sources.is_empty() {
            // An app no source is tied to. A report that arrived after it
            // took focus came from inside it: remember that.
            let r = self.latest_report(|r| r.at > w_at).cloned();
            if let Some(r) = &r {
                for id in w.ids() {
                    self.learned.insert(id, r.report.source.clone());
                }
            }
            r
        } else {
            self.latest_report(|r| sources.contains(&r.report.source))
                .cloned()
        };

        if let Some(r) = chosen {
            let ctx = self.contexts_from_report(&r.report);
            if !ctx.is_empty() {
                return self.set_sticky(ctx);
            }
        }

        let from_title = self.contexts_from_title(&w.title);
        if !from_title.is_empty() {
            return self.set_sticky(from_title);
        }

        let mut out = self.sticky.clone();
        let app = w.app_name();
        if !app.is_empty() {
            out.push(Context::App {
                app_id: app.to_owned(),
            });
        }
        out
    }

    fn set_sticky(&mut self, ctx: Vec<Context>) -> Vec<Context> {
        let files: Vec<Context> = ctx
            .iter()
            .filter(|c| matches!(c, Context::File { .. } | Context::Folder { .. }))
            .cloned()
            .collect();
        if !files.is_empty() {
            self.sticky = files;
        }
        ctx
    }

    fn latest_report(&self, keep: impl Fn(&Report) -> bool) -> Option<&Report> {
        self.reports
            .values()
            .filter(|r| !r.blurred && keep(r))
            .max_by_key(|r| r.at)
    }

    fn is_private(&self, w: &WindowFocus) -> bool {
        let title = w.title.trim().to_lowercase();
        self.apps.is_browser(w) && PRIVATE_TITLE_SUFFIXES.iter().any(|s| title.ends_with(s))
    }

    fn usable(&self, p: &Option<PathBuf>) -> Option<PathBuf> {
        p.as_ref()
            .filter(|p| p.is_absolute() && !self.ignore.path(p))
            .cloned()
    }

    /// Contexts for one report, most specific first. Empty if it names
    /// nothing usable.
    pub fn contexts_from_report(&self, r: &ContextReport) -> Vec<Context> {
        let root = self.usable(&r.project_root);
        if r.file.is_some() || r.cwd.is_some() || r.project_root.is_some() {
            // Anything in an ignored folder means no file at all.
            let ignored = [&r.file, &r.cwd, &r.project_root]
                .into_iter()
                .flatten()
                .any(|p| self.ignore.path(p));
            if ignored {
                return Vec::new();
            }
        }
        if let Some(file) = self.usable(&r.file) {
            let dir = file.parent().unwrap_or(Path::new("/"));
            return self.file_contexts(&file, dir, root.as_deref());
        }
        if let Some(root) = root {
            // A root the source names (Zed worktree, clawswap project) is used
            // as is (matching rule 5).
            return self.folder_contexts(root);
        }
        if let Some(cwd) = self.usable(&r.cwd) {
            let project = self.finder.project_for(&cwd, None);
            return self.folder_contexts(project);
        }
        if let Some(url) = r.url.as_deref().filter(|u| !u.is_empty())
            && !self.ignore.url(url)
        {
            return vec![Context::Url {
                url: url.to_owned(),
            }];
        }
        Vec::new()
    }

    fn file_contexts(&self, file: &Path, dir: &Path, hint: Option<&Path>) -> Vec<Context> {
        let project = self.finder.project_for(dir, hint);
        let rel = file.strip_prefix(&project).unwrap_or(file).to_path_buf();
        let mut out = vec![Context::File {
            project: project.clone(),
            file: rel,
        }];
        out.extend(self.folder_contexts(project));
        out
    }

    fn folder_contexts(&self, project: PathBuf) -> Vec<Context> {
        let parents = self.finder.parents_with_notes(&project);
        std::iter::once(project)
            .chain(parents)
            .map(|path| Context::Folder { path })
            .collect()
    }

    fn contexts_from_title(&self, title: &str) -> Vec<Context> {
        for p in title::candidates(title, self.home.as_deref()) {
            if self.ignore.path(&p) {
                return Vec::new();
            }
            if p.is_file() {
                let dir = p.parent().unwrap_or(Path::new("/"));
                return self.file_contexts(&p, dir, None);
            }
            if p.is_dir() {
                return self.folder_contexts(self.finder.project_for(&p, None));
            }
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::FsProjectFinder;
    use crate::project::tests::TempDir;
    use std::time::Duration;

    struct T {
        r: Resolver,
        t0: Instant,
        tick: u64,
    }

    impl T {
        fn new() -> Self {
            Self {
                r: Resolver::new(
                    Box::new(FsProjectFinder::default()),
                    AppTable::default(),
                    IgnoreList::default(),
                ),
                t0: Instant::now(),
                tick: 0,
            }
        }
        fn now(&mut self) -> Instant {
            self.tick += 1;
            self.t0 + Duration::from_millis(self.tick * 10)
        }
        fn window(&mut self, app_id: &str, title: &str) {
            let now = self.now();
            self.r.window_focus(
                WindowFocus {
                    app_id: app_id.into(),
                    wm_class: app_id.into(),
                    title: title.into(),
                    pid: 1,
                    ..Default::default()
                },
                now,
            );
        }
        fn report(&mut self, r: ContextReport) {
            let now = self.now();
            self.r.report(r, now);
        }
    }

    fn zed(root: &Path, file: &Path) -> ContextReport {
        ContextReport {
            source: Source::Zed,
            project_root: Some(root.into()),
            file: Some(file.into()),
            ..Default::default()
        }
    }

    fn zellij(cwd: &Path) -> ContextReport {
        ContextReport {
            source: Source::Zellij,
            cwd: Some(cwd.into()),
            session: Some("s".into()),
            ..Default::default()
        }
    }

    fn folder(p: PathBuf) -> Context {
        Context::Folder { path: p }
    }

    #[test]
    fn zed_file_resolves_to_project_and_parents() {
        let d = TempDir::new("res-zed");
        d.file("play/HITCH.md");
        d.dir("play/hitch/.git");
        let f = d.file("play/hitch/src/main.rs");
        let mut t = T::new();
        t.window("dev.zed.Zed", "main.rs — hitch");
        t.report(zed(&d.path("play/hitch"), &f));
        assert_eq!(
            t.r.resolve().contexts,
            vec![
                Context::File {
                    project: d.path("play/hitch"),
                    file: "src/main.rs".into()
                },
                folder(d.path("play/hitch")),
                folder(d.path("play")),
            ]
        );
    }

    #[test]
    fn last_file_stays_when_app_has_no_file() {
        let d = TempDir::new("res-sticky");
        d.dir("p/.git");
        let f = d.file("p/a.rs");
        let mut t = T::new();
        t.window("dev.zed.Zed", "");
        t.report(zed(&d.path("p"), &f));
        let first = t.r.resolve();
        t.r.blur(&Source::Zed);
        t.window("com.slack.Slack", "Slack | general");
        let now = t.r.resolve();
        assert_eq!(&now.contexts[..2], &first.contexts[..]);
        assert_eq!(
            now.contexts[2],
            Context::App {
                app_id: "com.slack.Slack".into()
            }
        );
    }

    #[test]
    fn plugin_report_only_used_while_its_app_is_frontmost() {
        let d = TempDir::new("res-front");
        d.dir("a/.git");
        d.dir("b/.git");
        let fa = d.file("a/x.rs");
        let mut t = T::new();
        // Zellij in a terminal reports b, then the user goes to Zed in a.
        t.window("org.gnome.Ptyxis", "");
        t.report(zellij(&d.path("b")));
        assert_eq!(t.r.resolve().contexts, vec![folder(d.path("b"))]);
        t.window("dev.zed.Zed", "");
        t.report(zed(&d.path("a"), &fa));
        assert_eq!(t.r.resolve().contexts[1], folder(d.path("a")));
        // Back to the terminal: Zellij's report applies again although
        // Zed's is newer.
        t.window("org.gnome.Ptyxis", "");
        assert_eq!(t.r.resolve().contexts, vec![folder(d.path("b"))]);
    }

    #[test]
    fn clawswap_needs_a_browser_and_skips_private_windows() {
        let d = TempDir::new("res-claw");
        let root = d.dir("site");
        let mut t = T::new();
        t.window("firefox_firefox", "Dev — Mozilla Firefox");
        t.report(ContextReport {
            source: Source::Clawswap,
            project_root: Some(root.clone()),
            ..Default::default()
        });
        assert_eq!(t.r.resolve().contexts, vec![folder(root.clone())]);
        let before = t.r.resolve();
        t.window("firefox", "Mozilla Firefox Private Browsing");
        assert_eq!(t.r.resolve(), before);
        // In Zed, clawswap's report doesn't apply.
        t.window("dev.zed.Zed", "");
        assert!(t.r.resolve().contexts.contains(&Context::App {
            app_id: "dev.zed.Zed".into()
        }));
    }

    #[test]
    fn unknown_terminal_is_learned() {
        let d = TempDir::new("res-learn");
        d.dir("p/.git");
        let mut t = T::new();
        t.window("org.example.NewTerm", "");
        t.report(zellij(&d.path("p")));
        assert_eq!(t.r.resolve().contexts, vec![folder(d.path("p"))]);
        t.window("other.App", "");
        t.r.resolve();
        t.window("org.example.NewTerm", "");
        assert_eq!(t.r.resolve().contexts, vec![folder(d.path("p"))]);
    }

    #[test]
    fn no_os_focus_uses_latest_report() {
        let d = TempDir::new("res-noos");
        d.dir("p/.git");
        d.dir("q/.git");
        let mut t = T::new();
        t.report(zellij(&d.path("p")));
        t.report(ContextReport {
            source: Source::Other("vscode".into()),
            cwd: Some(d.path("q")),
            ..Default::default()
        });
        assert_eq!(t.r.resolve().contexts, vec![folder(d.path("q"))]);
        t.r.blur(&Source::Other("vscode".into()));
        assert_eq!(t.r.resolve().contexts, vec![folder(d.path("p"))]);
        t.r.blur(&Source::Zellij);
        assert_eq!(t.r.resolve().contexts, vec![folder(d.path("p"))]);
    }

    #[test]
    fn ignore_list_applies_first() {
        let d = TempDir::new("res-ignore");
        d.dir("ok/.git");
        d.dir("secret/.git");
        let mut t = T::new();
        t.report(zellij(&d.path("ok")));
        let shown = t.r.resolve();
        t.r.set_ignore(IgnoreList {
            apps: vec!["org.keepassxc*".into()],
            folders: vec![d.path("secret")],
            urls: vec!["bank.example".into()],
        });
        t.report(zellij(&d.path("secret/sub")));
        assert_eq!(t.r.resolve(), shown);
        t.window("org.keepassxc.KeePassXC", "");
        assert_eq!(t.r.resolve(), shown);
        let r = &t.r;
        assert!(r.ignore.url("https://www.bank.example/login"));
        assert!(!r.ignore.url("https://notbank.example/"));
    }

    #[test]
    fn own_window_is_dropped() {
        let mut t = T::new();
        let pid = std::process::id();
        assert!(!t.r.window_focus(
            WindowFocus {
                app_id: "x".into(),
                pid,
                ..Default::default()
            },
            Instant::now()
        ));
        assert!(!t.r.window_focus(
            WindowFocus {
                app_id: hitch_protocol::APP_ID.into(),
                ..Default::default()
            },
            Instant::now()
        ));
        assert!(t.r.window.is_none());
    }

    #[test]
    fn title_path_for_other_apps() {
        let d = TempDir::new("res-title");
        d.dir("home/proj/.git");
        d.file("home/proj/notes.txt");
        let mut t = T::new();
        t.r.set_home(&d.path("home"));
        t.window("org.gnome.TextEditor", "notes.txt (~/proj) - Text Editor");
        assert_eq!(
            t.r.resolve().contexts[0],
            Context::File {
                project: d.path("home/proj"),
                file: "notes.txt".into()
            }
        );
    }

    #[test]
    fn zed_window_title_gives_the_file() {
        let d = TempDir::new("res-zedtitle");
        d.dir("hitch/.git");
        let f = d.file("hitch/src/main.rs");
        let mut t = T::new();
        t.window("dev.zed.Zed", &format!("hitch — {}", f.display()));
        assert_eq!(
            t.r.resolve().contexts[0],
            Context::File {
                project: d.path("hitch"),
                file: "src/main.rs".into()
            }
        );
        assert!(!t.r.take_zed_title_hint());
        // Remote or missing path: no file, the last one stays.
        t.window("dev.zed.Zed", "hitch — /no/such/remote/file.rs");
        assert_eq!(t.r.resolve().contexts[1], folder(d.path("hitch")));
        t.window("dev.zed.Zed", "hitch — main.rs");
        t.r.resolve();
        assert!(t.r.take_zed_title_hint());
        assert!(!t.r.take_zed_title_hint());
    }

    #[test]
    fn url_host_parsing() {
        assert_eq!(url_host("https://u:p@a.b:8080/x"), Some("a.b"));
        assert_eq!(url_host("http://[::1]/"), Some("[::1]"));
        assert_eq!(url_host("nope"), None);
    }
}
