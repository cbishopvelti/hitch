//! Well-known locations.

use std::path::PathBuf;

/// Environment variable that overrides the socket path (tests, several
/// instances).
pub const SOCKET_ENV: &str = "HITCH_SOCKET";

/// Hitch's own folder, `~/.hitch`.
pub fn hitch_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".hitch"))
}

/// The socket path: `$HITCH_SOCKET` if set, else `~/.hitch/hitch.sock`.
pub fn socket_path() -> Option<PathBuf> {
    match std::env::var_os(SOCKET_ENV) {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => hitch_dir().map(|d| d.join("hitch.sock")),
    }
}
