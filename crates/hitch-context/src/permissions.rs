//! OS permissions context detection needs, for the onboarding screen
//! (spec, UX, "Permissions onboarding").
//!
//! On macOS window titles need the Accessibility permission. Elsewhere
//! nothing is needed, so these report "granted" and do nothing.

/// Whether Hitch may read other apps' window titles.
pub fn accessibility_trusted() -> bool {
    #[cfg(target_os = "macos")]
    return hitch_macos::accessibility_trusted();
    #[cfg(not(target_os = "macos"))]
    true
}

/// Asks for the permission if Hitch doesn't have it: macOS shows its own
/// prompt, once per app. Returns whether Hitch has it now.
pub fn request_accessibility() -> bool {
    #[cfg(target_os = "macos")]
    return hitch_macos::request_accessibility();
    #[cfg(not(target_os = "macos"))]
    true
}

/// Opens System Settings at Privacy & Security > Accessibility, where the
/// user switches Hitch on.
pub fn open_accessibility_settings() {
    #[cfg(target_os = "macos")]
    hitch_macos::open_accessibility_settings();
}
