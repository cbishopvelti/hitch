//! macOS glue for Hitch (spec, "Architecture and platform", macOS column).
//!
//! - [`FocusWatcher`]: the frontmost app from NSWorkspace activation
//!   notifications, and its focused window's title and represented file
//!   (`AXDocument`) from the Accessibility API. Title changes inside an app
//!   (a tab switch in Zed) arrive through an `AXObserver`. Without the
//!   Accessibility permission only the app is known.
//! - [`set_keep_above`]: the pin. Floats Hitch's windows above others and
//!   shows them on every Space, including over full-screen apps.
//! - [`Hotkeys`]: global shortcuts through Carbon's `RegisterEventHotKey`
//!   (by way of the `global-hotkey` crate), which needs no permission.
//! - [`accessibility_trusted`], [`request_accessibility`] and
//!   [`open_accessibility_settings`] for onboarding.
//!
//! Cocoa and the Accessibility observer want the main thread. Everything
//! here hops to the main dispatch queue, which runs once the app's event
//! loop (iced's winit loop) is running; callbacks are made from there.
//!
//! On other platforms this crate is empty. It is the only crate in the
//! workspace that uses `unsafe`.

#[cfg(target_os = "macos")]
mod focus;
#[cfg(target_os = "macos")]
mod hotkey;
#[cfg(target_os = "macos")]
mod main_thread;
#[cfg(target_os = "macos")]
mod permission;
#[cfg(target_os = "macos")]
mod window;

#[cfg(target_os = "macos")]
pub use focus::{Focus, FocusEvent, FocusWatcher};
#[cfg(target_os = "macos")]
pub use hotkey::Hotkeys;
#[cfg(target_os = "macos")]
pub use permission::{accessibility_trusted, open_accessibility_settings, request_accessibility};
#[cfg(target_os = "macos")]
pub use window::set_keep_above;
