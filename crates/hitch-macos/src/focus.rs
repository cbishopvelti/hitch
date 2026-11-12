//! The frontmost app and its focused window.
//!
//! - App switches: `NSWorkspaceDidActivateApplicationNotification`.
//! - Window title and represented file: the Accessibility API on the
//!   frontmost app's focused window (`AXTitle`, `AXDocument`).
//! - Changes inside an app (another window, another tab): an `AXObserver`
//!   on the frontmost app for `AXFocusedWindowChanged`,
//!   `AXMainWindowChanged` and `AXTitleChanged`, its run loop source on the
//!   main run loop.
//!
//! All of it lives on the main thread. Titles are passed on, never stored
//! beyond the next change.

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchTime};
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_app_kit::{
    NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
    NSWorkspaceDidActivateApplicationNotification,
};
use objc2_application_services::{AXError, AXObserver, AXUIElement};
use objc2_core_foundation::{
    CFRetained, CFRunLoop, CFRunLoopSource, CFString, CFType, CFURL, kCFRunLoopCommonModes,
};
use objc2_foundation::NSNotification;

use crate::main_thread::on_main;
use crate::permission::accessibility_trusted;

/// How long an Accessibility call waits for a busy app before giving up.
/// They run on the main thread, so keep it short.
const AX_TIMEOUT_SECS: f32 = 0.25;

/// The frontmost app and, with the Accessibility permission, its focused
/// window. Fields are empty when unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Focus {
    /// Bundle identifier, e.g. `dev.zed.Zed`.
    pub bundle_id: String,
    /// The app's localized name, e.g. `Zed`.
    pub name: String,
    pub pid: i32,
    /// Title of the focused window.
    pub title: String,
    /// The file or folder the focused window represents (`AXDocument`),
    /// for apps that set it: Zed, TextEdit, Xcode, Preview, Terminal.
    pub document: Option<PathBuf>,
}

/// What a [`FocusWatcher`] reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusEvent {
    /// The frontmost app or its focused window changed. Not repeated while
    /// nothing changes.
    Focus(Focus),
    /// Whether Hitch has the Accessibility permission. Sent at start and
    /// whenever it changes (checked on each app switch).
    Accessibility(bool),
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// The running watcher. Main thread only.
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Watches the frontmost app and window. One per process: starting another
/// replaces the first. Stops when dropped.
#[derive(Debug)]
pub struct FocusWatcher {
    id: u64,
}

impl FocusWatcher {
    /// Starts watching. `on_event` is called on the main thread, once the
    /// app's event loop runs.
    pub fn start(on_event: impl Fn(FocusEvent) + Send + 'static) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let on_event: Box<dyn Fn(FocusEvent) + Send> = Box::new(on_event);
        on_main(move |_| start_on_main(id, on_event));
        Self { id }
    }
}

impl Drop for FocusWatcher {
    fn drop(&mut self) {
        let id = self.id;
        on_main(move |_| {
            let old = STATE.with(|s| {
                let mut s = s.borrow_mut();
                if s.as_ref().is_some_and(|st| st.id == id) {
                    s.take()
                } else {
                    None
                }
            });
            drop(old);
        });
    }
}

fn start_on_main(id: u64, on_event: Box<dyn Fn(FocusEvent) + Send>) {
    let block = RcBlock::new(|note: NonNull<NSNotification>| {
        // SAFETY: the notification centre passes a valid notification for
        // the duration of the call.
        let note = unsafe { note.as_ref() };
        let app =
            running_app(note).or_else(|| NSWorkspace::sharedWorkspace().frontmostApplication());
        if let Some(app) = app {
            with_state(|s| s.activate(&app));
        }
    });
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: the name is AppKit's constant. A nil queue runs the block on
    // the posting thread; NSWorkspace posts on the main thread, where
    // `STATE` lives.
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceDidActivateApplicationNotification),
            None,
            None,
            &block,
        )
    };

    let trusted = accessibility_trusted();
    let old = STATE.with(|s| {
        s.borrow_mut().replace(State {
            id,
            on_event,
            observer,
            trusted,
            app: None,
            ax: None,
            last: None,
        })
    });
    drop(old);

    with_state(|s| {
        (s.on_event)(FocusEvent::Accessibility(trusted));
        if let Some(app) = NSWorkspace::sharedWorkspace().frontmostApplication() {
            s.activate(&app);
        }
    });
}

/// The `NSRunningApplication` an activation notification is about.
fn running_app(note: &NSNotification) -> Option<Retained<NSRunningApplication>> {
    let info = note.userInfo()?;
    // SAFETY: AppKit's constant key.
    let key = unsafe { NSWorkspaceApplicationKey };
    info.objectForKey(key.as_ref())?
        .downcast::<NSRunningApplication>()
        .ok()
}

/// Runs `f` on the watcher's state, unless it is already borrowed (a
/// callback arriving while the state is being changed) or stopped.
fn with_state(f: impl FnOnce(&mut State)) {
    STATE.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut()
            && let Some(s) = s.as_mut()
        {
            f(s);
        }
    });
}

struct App {
    pid: i32,
    bundle_id: String,
    name: String,
}

struct State {
    id: u64,
    on_event: Box<dyn Fn(FocusEvent) + Send>,
    /// The NSWorkspace observer token, removed on drop.
    observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    trusted: bool,
    app: Option<App>,
    ax: Option<Ax>,
    last: Option<Focus>,
}

impl Drop for State {
    fn drop(&mut self) {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        // SAFETY: `observer` is the token this centre returned.
        unsafe { center.removeObserver(self.observer.as_ref()) };
    }
}

impl State {
    fn activate(&mut self, app: &NSRunningApplication) {
        let pid = app.processIdentifier();
        let trusted = accessibility_trusted();
        if trusted != self.trusted {
            self.trusted = trusted;
            (self.on_event)(FocusEvent::Accessibility(trusted));
        }
        let same_app = self.app.as_ref().is_some_and(|a| a.pid == pid);
        if !same_app || self.ax.is_none() {
            self.app = Some(App {
                pid,
                bundle_id: app
                    .bundleIdentifier()
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                name: app
                    .localizedName()
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
            });
            // Never observe Hitch itself: an Accessibility call from the main
            // thread to our own main thread would wait out the timeout.
            let own = u32::try_from(pid).ok() == Some(std::process::id());
            self.ax = None;
            if trusted && !own {
                self.ax = Ax::new(pid);
            }
        }
        self.refresh();
        // An app that is still launching may refuse the observer or have no
        // window yet, and then posts nothing. Look again a few times.
        if self.ax.as_ref().is_some_and(|ax| !ax.ready()) {
            for ms in RETRY_MS {
                let id = self.id;
                let _ = DispatchQueue::main().after(
                    DispatchTime::try_from(Duration::from_millis(ms)).unwrap_or(DispatchTime::NOW),
                    move || with_state(|s| s.retry(id, pid)),
                );
            }
        }
    }

    fn retry(&mut self, id: u64, pid: i32) {
        if self.id != id || self.app.as_ref().map(|a| a.pid) != Some(pid) {
            return;
        }
        if let Some(ax) = &mut self.ax {
            if ax.ready() {
                return;
            }
            ax.register();
        }
        self.refresh();
    }

    /// Reads the focused window and reports it if anything changed.
    fn refresh(&mut self) {
        let Some(app) = &self.app else { return };
        let mut focus = Focus {
            bundle_id: app.bundle_id.clone(),
            name: app.name.clone(),
            pid: app.pid,
            ..Default::default()
        };
        if let Some(ax) = &mut self.ax {
            ax.follow_focused_window();
            if let Some(window) = &ax.window {
                focus.title = string_attr(window, "AXTitle").unwrap_or_default();
                focus.document = document(window);
            }
        }
        if self.last.as_ref() != Some(&focus) {
            self.last = Some(focus.clone());
            (self.on_event)(FocusEvent::Focus(focus));
        }
    }
}

/// The Accessibility side for one app.
struct Ax {
    app: CFRetained<AXUIElement>,
    observer: CFRetained<AXObserver>,
    source: CFRetained<CFRunLoopSource>,
    /// The focused window, which also gets `AXTitleChanged`.
    window: Option<CFRetained<AXUIElement>>,
    /// Which of [`APP_NOTIFICATIONS`] are registered.
    registered: [bool; APP_NOTIFICATIONS.len()],
}

/// When to look again at an app that wasn't ready (milliseconds after it
/// was activated).
const RETRY_MS: [u64; 3] = [300, 1000, 3000];

/// `kAXErrorNotificationAlreadyRegistered`.
const ALREADY_REGISTERED: AXError = AXError(-25209);

/// Notifications watched on the app element.
const APP_NOTIFICATIONS: [&str; 3] = [
    "AXFocusedWindowChanged",
    "AXMainWindowChanged",
    "AXTitleChanged",
];

impl Ax {
    fn new(pid: i32) -> Option<Self> {
        // SAFETY: creating an element for a pid has no preconditions; a pid
        // that has gone away gives an element whose calls fail.
        let app = unsafe { AXUIElement::new_application(pid) };
        // SAFETY: `app` is a valid element.
        unsafe { app.set_messaging_timeout(AX_TIMEOUT_SECS) };

        let mut raw: *mut AXObserver = ptr::null_mut();
        // SAFETY: `ax_changed` has the AXObserverCallback signature and
        // `raw` is a valid out pointer.
        let err = unsafe { AXObserver::create(pid, Some(ax_changed), NonNull::from(&mut raw)) };
        if err != AXError::Success {
            return None;
        }
        // SAFETY: on success `raw` holds a +1 reference we now own.
        let observer = unsafe { CFRetained::from_raw(NonNull::new(raw)?) };
        // SAFETY: a valid observer.
        let source = unsafe { observer.run_loop_source() };
        // SAFETY: reading AppKit's constant run loop mode.
        let mode = unsafe { kCFRunLoopCommonModes };
        CFRunLoop::main()?.add_source(Some(&source), mode);
        let mut ax = Self {
            app,
            observer,
            source,
            window: None,
            registered: [false; APP_NOTIFICATIONS.len()],
        };
        ax.register();
        Some(ax)
    }

    /// Registers the app notifications not registered yet.
    fn register(&mut self) {
        for (name, done) in APP_NOTIFICATIONS.iter().zip(&mut self.registered) {
            if *done {
                continue;
            }
            // SAFETY: valid observer and element; the refcon is unused.
            let err = unsafe {
                self.observer.add_notification(
                    &self.app,
                    &CFString::from_static_str(name),
                    ptr::null_mut(),
                )
            };
            *done = err == AXError::Success || err == ALREADY_REGISTERED;
        }
    }

    /// Observing everything, with a window to read.
    fn ready(&self) -> bool {
        self.window.is_some() && self.registered.iter().all(|r| *r)
    }

    /// Moves the `AXTitleChanged` registration to the focused window, which
    /// some apps only post on the window itself.
    fn follow_focused_window(&mut self) {
        let window =
            attr(&self.app, "AXFocusedWindow").and_then(|v| v.downcast::<AXUIElement>().ok());
        if window == self.window {
            return;
        }
        let title_changed = CFString::from_static_str("AXTitleChanged");
        if let Some(old) = &self.window {
            // SAFETY: valid observer and element.
            let _ = unsafe { self.observer.remove_notification(old, &title_changed) };
        }
        if let Some(new) = &window {
            // SAFETY: valid observer and element; the refcon is unused.
            let _ = unsafe {
                self.observer
                    .add_notification(new, &title_changed, ptr::null_mut())
            };
        }
        self.window = window;
    }
}

impl Drop for Ax {
    fn drop(&mut self) {
        // SAFETY: reading AppKit's constant run loop mode.
        let mode = unsafe { kCFRunLoopCommonModes };
        if let Some(main) = CFRunLoop::main() {
            main.remove_source(Some(&self.source), mode);
        }
    }
}

/// Called on the main run loop for any watched Accessibility notification.
unsafe extern "C-unwind" fn ax_changed(
    _observer: NonNull<AXObserver>,
    _element: NonNull<AXUIElement>,
    _notification: NonNull<CFString>,
    _refcon: *mut c_void,
) {
    with_state(State::refresh);
}

fn attr(element: &AXUIElement, name: &'static str) -> Option<CFRetained<CFType>> {
    let mut value: *const CFType = ptr::null();
    // SAFETY: `value` is a valid out pointer.
    let err = unsafe {
        element.copy_attribute_value(&CFString::from_static_str(name), NonNull::from(&mut value))
    };
    if err != AXError::Success {
        return None;
    }
    // SAFETY: on success `value` holds a +1 reference we now own.
    NonNull::new(value.cast_mut()).map(|v| unsafe { CFRetained::from_raw(v) })
}

fn string_attr(element: &AXUIElement, name: &'static str) -> Option<String> {
    attr(element, name)?
        .downcast::<CFString>()
        .ok()
        .map(|s| s.to_string())
}

/// `AXDocument`: a `file://` URL, as a string or a CFURL.
fn document(window: &AXUIElement) -> Option<PathBuf> {
    let value = attr(window, "AXDocument")?;
    let url = match value.downcast::<CFURL>() {
        Ok(url) => url,
        Err(value) => {
            let s = value.downcast::<CFString>().ok()?;
            CFURL::from_string(None, &s, None)?
        }
    };
    url.to_file_path().filter(|p| p.is_absolute())
}
