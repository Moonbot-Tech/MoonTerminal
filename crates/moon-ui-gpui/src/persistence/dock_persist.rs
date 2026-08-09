//! Persistence for dock layouts, complementing the separate window-geometry persistence.
//!
//! MoonUI serializes a `DockArea` into serde-backed `DockAreaState` through `DockArea::dump` and
//! restores it through `DockArea::load` plus the application-wide `PanelRegistry`. A factory uses
//! the saved `panel_name` to rebuild each panel. Because the registry is global while each window
//! belongs to a different group, panels store their group and any other reconstruction parameters
//! in `PanelInfo::Panel` JSON from `Panel::dump`, and their factories read those values back. The
//! `group -> DockAreaState` map is stored at [`paths::docks_path`], currently `cfg/docks.json`
//! under the platform data directory.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use gpui::*;
use moon_ui::{DockAreaState, PanelInfo, PanelState, register_panel};

use moon_core::config::paths;

use crate::Backend;
use crate::chart_tabs::ChartTabs;
use crate::panels::DetectsPanel;
use crate::panels::registry;
use moon_core::session::CoreId;

/// Dock-layout schema version used to accept or reject saved layouts during restoration.
///
/// Increment this for incompatible panel-structure changes. A mismatched `docks.json` entry is
/// ignored and the group receives the default layout.
///
/// - v2 removed the Order panel containing the chart-side BUY/SELL buttons; resetting avoided a
///   missing-panel placeholder in layouts that still referenced it.
/// - v3 added the Alerts bottom tab and reset layouts so the new tab appeared.
/// - v4 added the Core Status bottom tab and reset layouts so the new tab appeared.
/// - v5 temporarily disabled Core Status because, at that time, log-derived data was insufficient
///   and typed moonproto fields were not yet available; resetting removed the previously saved tab.
/// - v6 added the News bottom tab and reset layouts so the new tab appeared.
/// - v7 restored the Core Status bottom tab with typed CPU and memory metrics.
/// - v8 moved the Log tab after News and Core Status.
pub const DOCK_VERSION: usize = 8;

/// Return whether a saved dock layout can be restored by the current panel schema.
///
/// # Arguments
/// * `version` - Schema version stored with the dock layout.
///
/// # Returns
/// `true` when the saved layout matches [`DOCK_VERSION`].
pub(crate) fn is_compatible_version(version: Option<usize>) -> bool {
    version == Some(DOCK_VERSION)
}

/// Map each group name to its serialized `DockArea` state.
pub type DockMap = HashMap<String, DockAreaState>;

/// Collect every `(group, panel_name)` that a restorable dock layout would put back in a dock.
///
/// This is the authority for "the panel is already in the dock", used to keep a stale
/// `DetachedSpec` from opening a second copy of a panel the dock will restore anyway —
/// `docks.json` and `detached.json` are debounced independently and can disagree after a repin
/// followed by a quick exit. A layout whose version no longer restores (see
/// [`is_compatible_version`]) contributes nothing: its panels will not appear, so they cannot
/// collide.
pub(crate) fn docked_panels(map: &DockMap) -> HashSet<(&str, &str)> {
    fn collect<'a>(node: &'a PanelState, group: &'a str, out: &mut HashSet<(&'a str, &'a str)>) {
        if matches!(node.info, PanelInfo::Panel(_)) && !node.panel_name.is_empty() {
            out.insert((group, node.panel_name.as_str()));
        }
        for c in &node.children {
            collect(c, group, out);
        }
    }
    let mut docked = HashSet::new();
    for (group, state) in map {
        if !is_compatible_version(state.version) {
            continue;
        }
        collect(&state.center, group, &mut docked);
        for d in [&state.left_dock, &state.right_dock, &state.bottom_dock]
            .into_iter()
            .flatten()
        {
            collect(&d.panel, group, &mut docked);
        }
    }
    docked
}

/// Load all dock layouts from `docks.json`.
///
/// A missing or unreadable file returns an empty map. A JSON deserialization failure emits a
/// warning and also returns an empty map, causing each group to build its default layout. Version
/// compatibility is checked later by the shell restoration path.
pub fn load_all() -> DockMap {
    match std::fs::read_to_string(paths::docks_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            log::warn!("docks.json битый ({e}) → дефолтная раскладка");
            DockMap::new()
        }),
        Err(_) => DockMap::new(),
    }
}

/// Serialize all dock layouts and atomically write an exact destination.
///
/// Args:
///     map: Complete Classic group-to-dock-state map.
///     path: Destination supplied by production or an isolated failure-path regression.
///
/// Returns:
///     `true` only after serialization and the atomic write both succeed.
pub(crate) fn save_all_to_path(map: &DockMap, path: &Path) -> bool {
    match serde_json::to_string_pretty(map) {
        Ok(s) => {
            if let Err(e) = moon_core::config::write_file_atomic(path, s.as_bytes(), "docks.json") {
                log::warn!("could not write docks.json: {e}");
                false
            } else {
                true
            }
        }
        Err(e) => {
            log::warn!("could not serialize docks.json: {e}");
            false
        }
    }
}

/// Read the panel group embedded by [`panel_state_with_group`].
///
/// Missing, non-panel, or non-string metadata falls back to the empty group name.
fn group_of(info: &PanelInfo) -> String {
    if let PanelInfo::Panel(v) = info {
        if let Some(g) = v.get("group").and_then(|g| g.as_str()) {
            return g.to_string();
        }
    }
    String::new()
}

/// Build the `PanelState` dumped by panels that require a group during reconstruction.
///
/// The state preserves `panel_name`, has no child panels, and stores `{"group": ...}` in
/// `PanelInfo::Panel`. The field name is part of the persisted JSON contract consumed by
/// [`group_of`].
pub fn panel_state_with_group(panel_name: &str, group: &str) -> PanelState {
    PanelState {
        panel_name: panel_name.to_string(),
        children: Vec::new(),
        info: PanelInfo::panel(serde_json::json!({ "group": group })),
    }
}

/// Register every dock-panel factory in the application-wide `PanelRegistry`.
///
/// Startup calls this once after creating the backend. Factories capture `backend` and `epoch`,
/// then recover the group and panel-specific view state from persisted panel metadata. Restored
/// ChartTabs intentionally start Main without opening a focus market automatically.
pub fn register_panels(cx: &mut App, backend: Entity<Backend>, epoch: f64) {
    // Chart tabs recover the group from state and obtain their theme from the backend.
    {
        let backend = backend.clone();
        register_panel(cx, "ChartTabs", move |_state, info, window, cx| {
            let group = group_of(info);
            let theme = backend.read(cx).config.chart_theme().clone();
            let backend = backend.clone();
            // Main starts empty; unlike a fresh group window, restoration opens no market.
            let focus: Option<(CoreId, String)> = None;
            Rc::new(cx.new(|cx| ChartTabs::new(backend, group, focus, epoch, theme, window, cx)))
        });
    }
    // The detect tape recovers its group from state and is constructed without a `Window`, so it
    // stays outside the shared registry alongside ChartTabs.
    {
        let backend = backend.clone();
        register_panel(cx, "Detects", move |_state, info, _window, cx| {
            let group = group_of(info);
            let backend = backend.clone();
            Rc::new(cx.new(|cx| DetectsPanel::new(backend, group, cx)))
        });
    }
    // Every detachable panel registers from the single registry: `build_docked` with persisted
    // `PanelInfo` reapplies saved view state (Orders' sort/kind/filter/columns) and recovers the
    // group.
    for kind in registry::DOCK_PANELS {
        let backend = backend.clone();
        register_panel(cx, kind.name, move |_state, info, window, cx| {
            let group = group_of(info);
            kind.build_docked(&backend, &group, Some(info), window, cx)
        });
    }
}

#[cfg(test)]
mod tests;
