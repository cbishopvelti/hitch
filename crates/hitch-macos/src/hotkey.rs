//! Global shortcuts (spec, "Global shortcut: native global hotkey API").
//!
//! Carbon's `RegisterEventHotKey`, through the `global-hotkey` crate. It
//! needs no permission, and works while Hitch is in the background.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex, Once};

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::main_thread::on_main;

type Action = Arc<dyn Fn() + Send + Sync>;

/// What each registered hot key id does. `global-hotkey` takes one event
/// handler per process, so it is installed once and dispatches from here.
static ACTIONS: Mutex<Option<HashMap<u32, Action>>> = Mutex::new(None);
static HANDLER: Once = Once::new();

thread_local! {
    /// Created on the main thread, as Carbon needs. Main thread only.
    static MANAGER: std::cell::RefCell<Option<GlobalHotKeyManager>> =
        const { std::cell::RefCell::new(None) };
}

fn actions() -> std::sync::MutexGuard<'static, Option<HashMap<u32, Action>>> {
    ACTIONS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Registered global shortcuts. Unregistered when dropped.
pub struct Hotkeys {
    keys: Vec<HotKey>,
}

impl std::fmt::Debug for Hotkeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hotkeys")
            .field("count", &self.keys.len())
            .finish()
    }
}

impl Hotkeys {
    /// Registers each `(shortcut, action)`. Shortcuts are written like
    /// `Cmd+Shift+N` (`Ctrl`, `Alt`/`Option`, `Shift`, `Cmd`, then a key
    /// such as `N`, `F5` or `Space`). Returns an error for one that can't be
    /// parsed. Registration itself happens on the main thread; a shortcut
    /// another app already holds is passed to `on_unavailable` from there.
    pub fn register(
        shortcuts: Vec<(String, Box<dyn Fn() + Send + Sync>)>,
        on_unavailable: impl Fn(String) + Send + 'static,
    ) -> Result<Self, String> {
        let mut parsed = Vec::new();
        for (text, action) in shortcuts {
            let key = HotKey::from_str(&text).map_err(|e| format!("shortcut {text:?}: {e}"))?;
            parsed.push((text, key, Arc::<dyn Fn() + Send + Sync>::from(action)));
        }

        HANDLER.call_once(|| {
            GlobalHotKeyEvent::set_event_handler(Some(|event: GlobalHotKeyEvent| {
                if event.state() != HotKeyState::Pressed {
                    return;
                }
                let action = actions().as_ref().and_then(|a| a.get(&event.id()).cloned());
                if let Some(action) = action {
                    action();
                }
            }));
        });

        let keys: Vec<HotKey> = parsed.iter().map(|(_, k, _)| *k).collect();
        {
            let mut table = actions();
            let table = table.get_or_insert_with(HashMap::new);
            for (_, key, action) in &parsed {
                table.insert(key.id(), action.clone());
            }
        }

        on_main(move |_| {
            MANAGER.with(|m| {
                let mut m = m.borrow_mut();
                if m.is_none() {
                    match GlobalHotKeyManager::new() {
                        Ok(manager) => *m = Some(manager),
                        Err(_) => {
                            for (text, _, _) in &parsed {
                                on_unavailable(text.clone());
                            }
                            return;
                        }
                    }
                }
                let Some(manager) = m.as_ref() else { return };
                for (text, key, _) in &parsed {
                    if manager.register(*key).is_err() {
                        on_unavailable(text.clone());
                    }
                }
            });
        });

        Ok(Self { keys })
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        if let Some(table) = actions().as_mut() {
            for key in &self.keys {
                table.remove(&key.id());
            }
        }
        let keys = std::mem::take(&mut self.keys);
        on_main(move |_| {
            MANAGER.with(|m| {
                if let Some(manager) = m.borrow().as_ref() {
                    let _ = manager.unregister_all(&keys);
                }
            });
        });
    }
}
