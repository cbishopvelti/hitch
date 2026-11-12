//! The pin: Hitch's windows float above other apps and show on every Space.
//!
//! iced's `Level::AlwaysOnTop` sets the floating window level, but a window
//! still stays on the Space it was opened on. The spec wants a pinned panel
//! "on every desktop or Space", so this also sets the window's collection
//! behaviour. Windows opened later (the panel is re-created each time it is
//! shown) get the same treatment when they become key.

use std::cell::Cell;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_app_kit::{
    NSApplication, NSFloatingWindowLevel, NSNormalWindowLevel, NSWindow,
    NSWindowCollectionBehavior, NSWindowDidBecomeKeyNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter};

use crate::main_thread::on_main;

static PINNED: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Set once the first pin call registered the key-window observer.
    /// Main thread only.
    static OBSERVING: Cell<bool> = const { Cell::new(false) };
}

/// Pin (`true`) or unpin Hitch's windows. Applies to every open window now
/// and to each window that becomes key later. Returns at once; the change
/// is made on the main thread.
pub fn set_keep_above(pinned: bool) {
    PINNED.store(pinned, Ordering::SeqCst);
    on_main(move |mtm| {
        observe_key_windows();
        // The app object exists by now: this runs on the main queue, which
        // only drains once winit has created it and is running.
        let app = NSApplication::sharedApplication(mtm);
        for window in app.windows().iter() {
            apply(&window, pinned);
        }
    });
}

fn apply(window: &NSWindow, pinned: bool) {
    let every_space = NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::FullScreenAuxiliary;
    let mut behavior = window.collectionBehavior();
    if pinned {
        // CanJoinAllSpaces and MoveToActiveSpace exclude each other.
        behavior.remove(NSWindowCollectionBehavior::MoveToActiveSpace);
        behavior.insert(every_space);
    } else {
        behavior.remove(every_space);
    }
    window.setCollectionBehavior(behavior);
    window.setLevel(if pinned {
        NSFloatingWindowLevel
    } else {
        NSNormalWindowLevel
    });
}

/// Registers, once, for `NSWindowDidBecomeKeyNotification` so windows
/// opened after the pin get it too.
fn observe_key_windows() {
    if OBSERVING.with(|o| o.replace(true)) {
        return;
    }
    let block = RcBlock::new(|note: NonNull<NSNotification>| {
        // SAFETY: the notification centre passes a valid notification for
        // the duration of the call.
        let note = unsafe { note.as_ref() };
        let Some(window) = note.object().and_then(|o| o.downcast::<NSWindow>().ok()) else {
            return;
        };
        apply(&window, PINNED.load(Ordering::SeqCst));
    });
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: the name is AppKit's constant; a nil queue runs the block on
    // the posting thread, which for window notifications is the main thread,
    // as `apply` needs.
    let token: Retained<ProtocolObject<dyn NSObjectProtocol>> = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWindowDidBecomeKeyNotification),
            None,
            None,
            &block,
        )
    };
    // The observer lives as long as the app.
    std::mem::forget(token);
}
