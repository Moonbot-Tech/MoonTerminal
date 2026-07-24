//! Persistence for dock layouts, complementing the separate window-geometry persistence.
//!
//! MoonUI serializes a `DockArea` into serde-backed `DockAreaState` through `DockArea::dump` and
//! restores it through `DockArea::load` plus the application-wide `PanelRegistry`. A factory uses
//! the saved `panel_name` to rebuild each panel. Because the registry is global while each window
//! belongs to a different group, panels store their group and any other reconstruction parameters
//! in `PanelInfo::Panel` JSON from `Panel::dump`, and their factories read those values back. The
//! `group -> DockAreaState` map is stored at [`paths::docks_path`], currently `cfg/docks.json`
//! under the platform data directory.

use std::collections::HashMap;
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
pub const DOCK_VERSION: usize = 5;

/// Map each group name to its serialized `DockArea` state.
pub type DockMap = HashMap<String, DockAreaState>;

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

/// Serialize all dock layouts as pretty JSON and atomically write `docks.json`.
///
/// Serialization and write failures are non-fatal and produce warnings only.
pub fn save_all(map: &DockMap) {
    match serde_json::to_string_pretty(map) {
        Ok(s) => {
            if let Err(e) = moon_core::config::write_file_atomic(
                &paths::docks_path(),
                s.as_bytes(),
                "docks.json",
            ) {
                log::warn!("не записал docks.json: {e}");
            }
        }
        Err(e) => log::warn!("не сериализовал docks.json: {e}"),
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
    // Every detachable panel (Orders, Assets, Report, Alerts, Log, CoreStatus) registers from the
    // single registry: `build_docked` with the persisted `PanelInfo` reapplies saved view state
    // (Orders' sort/kind/filter/columns) and recovers the group.
    for kind in registry::DOCK_PANELS {
        let backend = backend.clone();
        register_panel(cx, kind.name, move |_state, info, window, cx| {
            let group = group_of(info);
            kind.build_docked(&backend, &group, Some(info), window, cx)
        });
    }
}
