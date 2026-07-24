//! Localized dock-panel labels — the single `panel_name` → `t!("dock.tab.*")` map.
//!
//! `panel_name()` is the STABLE IDENTITY of a panel: layout persistence (`dock_persist`), the
//! detachable-panel registry (`panels::registry`, which also feeds the home strip in
//! `shell::docks`), and the detached-window specs (`detached`) all match on it, and MoonUI's dock
//! does the same internally. It stays an English literal and is never translated.
//!
//! What the user actually reads lives here, and all three surfaces route through it: the dock tab
//! (`Panel::tab_name`), the panel header (`Panel::title`), and the detached-window title. Keeping
//! one map ensures that a panel cannot be localized on one surface and remain English on another.

use gpui::SharedString;
use rust_i18n::t;

/// The localized label for a panel, or `None` when the name is not a known dock panel.
///
/// `None` is meaningful: MoonUI falls back to `panel_name()` for the tab label, which is the right
/// behaviour for runtime-named panels (the stub) and for surfaces that carry their own title.
pub(crate) fn tab_label(name: &str) -> Option<SharedString> {
    let label = match name {
        "Orders" => t!("dock.tab.orders"),
        "Assets" => t!("dock.tab.assets"),
        "Log" => t!("dock.tab.log"),
        "News" => t!("dock.tab.news"),
        "Report" => t!("dock.tab.report"),
        "Alerts" => t!("dock.tab.alerts"),
        "Detects" => t!("dock.tab.detects"),
        "CoreStatus" => t!("dock.tab.core_status"),
        "ChartTabs" => t!("dock.tab.charts"),
        _ => return None,
    };
    Some(SharedString::from(label.to_string()))
}

/// The same label with the neutral fallback that a titled surface always needs — used by the
/// panel header (`Panel::title`), the detached window's OS title, and the Assets tool-window
/// titlebar.
///
/// Separate from [`tab_label`] because the two disagree on what a miss means: a dock tab falls back
/// to `panel_name()`, which is right for a runtime-named panel, while a window with no title at all
/// is not.
///
/// Returns `SharedString` rather than `String`: the panel-header callers render it every frame and
/// would otherwise pay an unwrap-and-rewrap round trip, while the one caller that genuinely needs an
/// owned `String` (the OS window title) runs once at window open.
pub(crate) fn panel_title(name: &str) -> SharedString {
    tab_label(name).unwrap_or_else(|| SharedString::from(t!("dock.tab.generic").to_string()))
}
