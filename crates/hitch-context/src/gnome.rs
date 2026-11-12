//! The GNOME Shell extension over D-Bus (`docs/PROTOCOL.md`, "GNOME Shell
//! extension"). On Wayland only the shell knows the focused window.
//!
//! Hitch subscribes to `FocusChanged` and calls `GetFocus` whenever the
//! extension appears on the bus, so the extension needs no reconnect logic.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use zbus::blocking::{Connection, Proxy, fdo::DBusProxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::OwnedValue;

use crate::engine::InputSink;
use crate::resolve::WindowFocus;

pub const BUS_NAME: &str = "io.github.cbishopvelti.Hitch.Shell";
pub const OBJECT_PATH: &str = "/io/github/cbishopvelti/Hitch/Shell";
pub const INTERFACE: &str = "io.github.cbishopvelti.Hitch.Shell";

/// A live subscription. Holds the D-Bus connection, which also keeps
/// `SetKeepAbove` in force: the extension drops the pin when the caller
/// leaves the bus.
pub(crate) struct Shell {
    proxy: Proxy<'static>,
    /// Whether Hitch asked to be pinned. Applied again when the extension
    /// comes back (GNOME disables extensions while the screen is locked).
    pinned: Arc<AtomicBool>,
}

impl Shell {
    /// Connect to the session bus (or `address`) and start watching.
    pub(crate) fn start(address: Option<&str>, sink: InputSink) -> zbus::Result<Self> {
        let conn = match address {
            Some(a) => zbus::blocking::connection::Builder::address(a)?.build()?,
            None => Connection::session()?,
        };
        let proxy: Proxy<'static> = zbus::blocking::proxy::Builder::new(&conn)
            .destination(BUS_NAME)?
            .path(OBJECT_PATH)?
            .interface(INTERFACE)?
            .cache_properties(CacheProperties::No)
            .build()?;

        // Subscribe before asking for the current state, so nothing falls
        // in between.
        let signals = proxy.receive_signal("FocusChanged")?;
        let owners = proxy.receive_owner_changed()?;

        let present = DBusProxy::new(&conn)?.name_has_owner(BUS_NAME.try_into()?)?;
        sink.shell_available(present);
        if present {
            refresh(&proxy, &sink);
        }

        let s = sink.clone();
        thread::Builder::new()
            .name("hitch-gnome-focus".into())
            .spawn(move || {
                for msg in signals {
                    if let Ok(map) = msg.body().deserialize::<HashMap<String, OwnedValue>>() {
                        s.window_focus(focus_from_map(&map));
                    }
                }
            })
            .map_err(|e| zbus::Error::InputOutput(e.into()))?;

        let pinned = Arc::new(AtomicBool::new(false));
        let (p, pin) = (proxy.clone(), pinned.clone());
        thread::Builder::new()
            .name("hitch-gnome-owner".into())
            .spawn(move || {
                for owner in owners {
                    let present = owner.is_some();
                    sink.shell_available(present);
                    if present {
                        refresh(&p, &sink);
                        if pin.load(Ordering::SeqCst) {
                            let _ = p.call::<_, _, ()>("SetKeepAbove", &(true,));
                        }
                    } else {
                        sink.window_gone();
                    }
                }
            })
            .map_err(|e| zbus::Error::InputOutput(e.into()))?;

        Ok(Self { proxy, pinned })
    }

    /// Keep Hitch's windows above others and on every workspace.
    /// Remembered even if the call fails, and applied again whenever the
    /// extension appears.
    pub(crate) fn set_keep_above(&self, pinned: bool) -> zbus::Result<()> {
        self.pinned.store(pinned, Ordering::SeqCst);
        self.proxy.call::<_, _, ()>("SetKeepAbove", &(pinned,))
    }
}

fn refresh(proxy: &Proxy<'_>, sink: &InputSink) {
    if let Ok(map) = proxy.call::<_, _, HashMap<String, OwnedValue>>("GetFocus", &()) {
        sink.window_focus(focus_from_map(&map));
    }
}

fn focus_from_map(map: &HashMap<String, OwnedValue>) -> WindowFocus {
    let s = |k: &str| {
        map.get(k)
            .and_then(|v| v.downcast_ref::<&str>().ok())
            .unwrap_or_default()
            .to_owned()
    };
    WindowFocus {
        app_id: s("app_id"),
        wm_class: s("wm_class"),
        title: s("title"),
        pid: map
            .get("pid")
            .and_then(|v| v.downcast_ref::<u32>().ok())
            .unwrap_or(0),
        sandboxed_app_id: s("sandboxed_app_id"),
    }
}
