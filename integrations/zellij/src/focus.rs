//! Which terminal pane has focus, worked out from Zellij's tab and pane
//! updates.
//!
//! Zellij tells plugins about every tab ([`TabInfo`]) and every pane per tab
//! ([`PaneManifest`]). Each pane says whether it is focused *in its layer*
//! (tiled or floating), so the focused pane of the session is the focused
//! pane of the active tab's visible layer: floating if the tab shows its
//! floating panes, tiled otherwise.

use zellij_tile::prelude::{PaneManifest, TabInfo};

#[derive(Debug, Default)]
pub struct FocusTracker {
    /// Position of the active tab, and whether it shows floating panes.
    active_tab: Option<(usize, bool)>,
    manifest: PaneManifest,
    /// Terminal pane focused right now, `None` when a plugin pane (or
    /// nothing) has focus.
    focused: Option<u32>,
    /// Last terminal pane that had focus. Commands from Hitch are typed here,
    /// so focusing a plugin pane doesn't lose the target.
    last_terminal: Option<u32>,
}

impl FocusTracker {
    pub fn on_tabs(&mut self, tabs: &[TabInfo]) -> bool {
        self.active_tab = tabs
            .iter()
            .find(|t| t.active)
            .map(|t| (t.position, t.are_floating_panes_visible));
        self.recompute()
    }

    pub fn on_panes(&mut self, manifest: PaneManifest) -> bool {
        self.manifest = manifest;
        self.recompute()
    }

    /// The terminal pane focused now.
    pub fn focused(&self) -> Option<u32> {
        self.focused
    }

    /// Where to type a command: the last focused terminal pane.
    pub fn target(&self) -> Option<u32> {
        self.last_terminal
    }

    /// A pane closed. Forget it so commands aren't typed into nothing.
    pub fn on_pane_closed(&mut self, id: u32) {
        if self.last_terminal == Some(id) {
            self.last_terminal = None;
        }
        if self.focused == Some(id) {
            self.focused = None;
        }
    }

    /// Returns true when the focused terminal pane changed.
    fn recompute(&mut self) -> bool {
        let Some((tab, floating_visible)) = self.active_tab else {
            return false;
        };
        let Some(panes) = self.manifest.panes.get(&tab) else {
            return false;
        };
        let focused = panes
            .iter()
            .filter(|p| p.is_focused && !p.is_suppressed)
            .find(|p| p.is_floating == floating_visible)
            // A tab with visible floating panes but none focused (all closed
            // since) falls back to the tiled layer.
            .or_else(|| {
                panes
                    .iter()
                    .find(|p| p.is_focused && !p.is_suppressed && !p.is_floating)
            });
        let terminal = focused.filter(|p| !p.is_plugin).map(|p| p.id);
        if let Some(id) = terminal {
            self.last_terminal = Some(id);
        }
        let changed = terminal != self.focused;
        self.focused = terminal;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use zellij_tile::prelude::PaneInfo;

    fn tab(position: usize, active: bool, floating: bool) -> TabInfo {
        TabInfo {
            position,
            active,
            are_floating_panes_visible: floating,
            ..Default::default()
        }
    }

    fn pane(id: u32, focused: bool, floating: bool, plugin: bool) -> PaneInfo {
        PaneInfo {
            id,
            is_focused: focused,
            is_floating: floating,
            is_plugin: plugin,
            ..Default::default()
        }
    }

    fn manifest(tabs: Vec<(usize, Vec<PaneInfo>)>) -> PaneManifest {
        PaneManifest {
            panes: tabs.into_iter().collect::<HashMap<_, _>>(),
        }
    }

    #[test]
    fn tiled_focus_in_active_tab() {
        let mut f = FocusTracker::default();
        f.on_tabs(&[tab(0, false, false), tab(1, true, false)]);
        let changed = f.on_panes(manifest(vec![
            (0, vec![pane(1, true, false, false)]),
            (
                1,
                vec![pane(2, false, false, false), pane(3, true, false, false)],
            ),
        ]));
        assert!(changed);
        assert_eq!(f.focused(), Some(3));
        assert_eq!(f.target(), Some(3));
    }

    #[test]
    fn floating_layer_wins_when_visible() {
        let mut f = FocusTracker::default();
        let panes = vec![pane(1, true, false, false), pane(4, true, true, false)];
        f.on_tabs(&[tab(0, true, true)]);
        f.on_panes(manifest(vec![(0, panes.clone())]));
        assert_eq!(f.focused(), Some(4));
        // Hiding the floating panes moves focus back to the tiled pane.
        assert!(f.on_tabs(&[tab(0, true, false)]));
        assert_eq!(f.focused(), Some(1));
    }

    #[test]
    fn plugin_focus_keeps_last_terminal_as_target() {
        let mut f = FocusTracker::default();
        f.on_tabs(&[tab(0, true, false)]);
        f.on_panes(manifest(vec![(0, vec![pane(1, true, false, false)])]));
        let changed = f.on_panes(manifest(vec![(
            0,
            vec![pane(1, false, false, false), pane(9, true, false, true)],
        )]));
        assert!(changed);
        assert_eq!(f.focused(), None);
        assert_eq!(f.target(), Some(1));
    }

    #[test]
    fn switching_tabs_changes_focus() {
        let mut f = FocusTracker::default();
        let m = manifest(vec![
            (0, vec![pane(1, true, false, false)]),
            (1, vec![pane(2, true, false, false)]),
        ]);
        f.on_tabs(&[tab(0, true, false), tab(1, false, false)]);
        f.on_panes(m);
        assert_eq!(f.focused(), Some(1));
        assert!(f.on_tabs(&[tab(0, false, false), tab(1, true, false)]));
        assert_eq!(f.focused(), Some(2));
        assert!(!f.on_tabs(&[tab(0, false, false), tab(1, true, false)]));
    }

    #[test]
    fn closed_pane_is_no_longer_a_target() {
        let mut f = FocusTracker::default();
        f.on_tabs(&[tab(0, true, false)]);
        f.on_panes(manifest(vec![(0, vec![pane(5, true, false, false)])]));
        f.on_pane_closed(5);
        assert_eq!(f.target(), None);
        assert_eq!(f.focused(), None);
    }
}
