//! One-shot carry-over of the retired app-wide order-book width into the per-tab setting.
//!
//! The book width used to be one number in `settings.toml`, edited on the Settings appearance
//! page, and every chart drew it. It is a field of `ChartGraphicsCfg` now, edited from the "Chart
//! graphics" popup per tab and per kind of tab. Without this pass every user who had widened the
//! book would watch it snap back to the shipped 220 px on every chart.
//!
//! The pass writes the stored width everywhere a tab can read one from: the base default, every
//! stored per-kind default and every tab override in `charts.json`. The overrides are not a guess:
//! a tab holding graphics of its own drew the global width like any other.
//!
//! Guarded by `WindowLayout::chart_book_width_migrated`. It neither sets that marker nor saves
//! anything: both are the caller's, which sets the marker together with this stamp and never
//! defers it (see the call in `startup.rs` for why). A second pass could overwrite a width set
//! per tab since — the retired key stays in `settings.toml` until that file is next saved.

use moon_core::config::book_width;
use moon_core::config::layout::WindowLayout;

use crate::persistence::chart_persist::ChartTabSpec;

/// What one pass changed, so the caller writes only the files it has to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Carried {
    /// Whether any tab spec was rewritten, and `charts.json` therefore has to be saved.
    pub specs_changed: bool,
}

/// Fold the retired app-wide width into the per-tab setting wherever a tab reads it from.
///
/// Args:
///     layout: The loaded window layout — its base graphics default and its per-kind defaults.
///     specs: Every loaded chart tab spec; only a tab with its own graphics override is rewritten.
///     legacy: The width `settings.toml` still holds, already normalized, or `None` without one.
///
/// Returns:
///     What the pass did, or `None` when it had already run.
pub(super) fn migrate_book_width(
    layout: &mut WindowLayout,
    specs: &mut [ChartTabSpec],
    legacy: Option<f32>,
) -> Option<Carried> {
    if layout.chart_book_width_migrated {
        return None;
    }
    let mut carried = Carried::default();
    let Some(width) = legacy.filter(|w| *w != book_width::DEFAULT) else {
        // No stored width, or the shipped one, which is what every per-tab field already reads.
        // Nothing to write; the marker still gets committed.
        return Some(carried);
    };
    for cfg in layout.stored_chart_graphics_mut() {
        cfg.book_width_px = width;
    }
    for spec in specs.iter_mut() {
        if let Some(cfg) = spec.chart_graphics.as_mut()
            && cfg.book_width_px != width
        {
            cfg.book_width_px = width;
            carried.specs_changed = true;
        }
    }
    Some(carried)
}

#[cfg(test)]
mod tests;
