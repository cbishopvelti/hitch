//! The Accessibility permission, which window titles need.
//!
//! macOS ties the permission to the app the user launched: `Hitch.app`, or
//! the terminal when `hitch` is run from one.

use objc2_app_kit::NSWorkspace;
use objc2_application_services::{
    AXIsProcessTrusted, AXIsProcessTrustedWithOptions, kAXTrustedCheckOptionPrompt,
};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFString, CFType};
use objc2_foundation::{NSString, NSURL};

use crate::main_thread::on_main;

/// The Privacy & Security > Accessibility pane of System Settings.
const ACCESSIBILITY_SETTINGS: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

/// Whether Hitch may read other apps' windows.
pub fn accessibility_trusted() -> bool {
    // SAFETY: takes no arguments and only reads the process's TCC state.
    unsafe { AXIsProcessTrusted() }
}

/// Like [`accessibility_trusted`], and if not, asks macOS to show its
/// prompt, which offers to open System Settings. macOS shows the prompt
/// once per app; after that use [`open_accessibility_settings`].
pub fn request_accessibility() -> bool {
    // SAFETY: the key is a constant CFString exported by HIServices.
    let key: &CFString = unsafe { kAXTrustedCheckOptionPrompt };
    let yes: &CFBoolean = CFBoolean::new(true);
    let options = CFDictionary::<CFString, CFType>::from_slices(&[key], &[yes.as_ref()]);
    // SAFETY: `options` is a valid dictionary of the documented key and a
    // CFBoolean value.
    unsafe { AXIsProcessTrustedWithOptions(Some(options.as_opaque())) }
}

/// Opens System Settings at Privacy & Security > Accessibility.
pub fn open_accessibility_settings() {
    on_main(|_| {
        if let Some(url) = NSURL::URLWithString(&NSString::from_str(ACCESSIBILITY_SETTINGS)) {
            NSWorkspace::sharedWorkspace().openURL(&url);
        }
    });
}
