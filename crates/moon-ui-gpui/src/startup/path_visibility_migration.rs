//! One-shot carry-over of the retired global "show path" toggle into the per-tab flag (#612).
//!
//! The repricing history of an order line used to answer to two switches joined by AND: the
//! per-tab `ChartGraphicsCfg::hide_order_move_history` and a global `PathStyle::show` in
//! `orders.toml`, kept separately for the light and the dark set. The global one is gone from the
//! UI and from the draw sites. Without this pass every user who had turned it off would watch the
//! path silently return on every tab.
//!
//! The pass reads what the user had chosen and, when the path was hidden, sets the per-tab flag on
//! everything a tab can open with: the base default, every stored per-kind default and every tab
//! override in `charts.json`. The overrides are not a guess: a tab holding `Some(cfg)` drew the
//! global veto like any other, so writing the flag in restores exactly what it drew.
//!
//! "Hidden" means EITHER theme set had the toggle off — a decision of #612, not a description of
//! the old runtime. The old draw site read the set of the theme live at the moment, so a toggle
//! unticked in dark hid the path in dark only, and it came back on a switch to light; the issue
//! names exactly that as the trap this change removes. Unticking "show path" was the user's choice
//! to hide the path; that it landed in one theme's table was an accident of where it was stored.
//! `theme_legacy::active_theme_mode` resolves values that ARE per theme, which this one is not.
//!
//! Guarded by `WindowLayout::chart_path_visibility_migrated`. It neither sets that marker nor saves
//! anything: both are the caller's, because the marker may only be committed once the tab specs
//! are known to be on disk. A second pass would re-hide the history on every tab the user has since
//! shown it on — the retired flag stays in `orders.toml`, so the source never runs dry.

use moon_core::config::OrdersStyleSet;
use moon_core::config::layout::WindowLayout;

use crate::persistence::chart_persist::ChartTabSpec;

/// Whether the retired toggle hid the path in either theme set — see the module doc for why
/// either, not the theme active at this launch.
fn path_hidden(orders: &OrdersStyleSet) -> bool {
    !orders.dark.path.show || !orders.light.path.show
}

/// What one pass changed, so the caller writes only the files it has to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Carried {
    /// Whether any tab spec was rewritten, and `charts.json` therefore has to be saved.
    pub specs_changed: bool,
}

/// Fold the retired global toggle into the per-tab flag wherever a tab reads it from.
///
/// Args:
///     layout: The loaded window layout — its base graphics default and its per-kind defaults.
///     specs: Every loaded chart tab spec; only a tab with its own graphics override is rewritten.
///     orders: The order-line styles as `orders.toml` holds them, both theme sets.
///
/// Returns:
///     What the pass did, or `None` when it had already run.
pub(super) fn migrate_path_visibility(
    layout: &mut WindowLayout,
    specs: &mut [ChartTabSpec],
    orders: &OrdersStyleSet,
) -> Option<Carried> {
    if layout.chart_path_visibility_migrated {
        return None;
    }
    let mut carried = Carried::default();
    if !path_hidden(orders) {
        // The path was on, which is what every per-tab flag already says by default — and a tab
        // that hid it on its own keeps that. Nothing to write; the marker still gets committed.
        return Some(carried);
    }
    for cfg in layout.stored_chart_graphics_mut() {
        cfg.hide_order_move_history = true;
    }
    for spec in specs.iter_mut() {
        if let Some(cfg) = spec.chart_graphics.as_mut()
            && !cfg.hide_order_move_history
        {
            cfg.hide_order_move_history = true;
            carried.specs_changed = true;
        }
    }
    Some(carried)
}

#[cfg(test)]
mod tests;
