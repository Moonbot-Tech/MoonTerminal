//! One-shot insertion of the strategy-filters caption into existing caption sets.
//!
//! The skip-reason overlay used to be a graphics checkbox. It is a caption module now, placed like
//! everything else the chart prints, so a profile written before that module existed would keep
//! drawing none of it until the reader added it by hand.
//!
//! The pass appends the shipped row — ChartTop, left, column, gap 8 — to every live caption set
//! that does not already place one. Compare and Trade are left alone: their shipped sets never
//! included it, and a skip-reason column on a frozen trade or a comparison pane is not what those
//! views are read by.
//!
//! Guarded by `WindowLayout::chart_strategy_filters_migrated`. It neither sets that marker nor
//! saves anything: both are the caller's, because the marker may only be committed once the tab
//! specs are known to be on disk. A second pass would re-insert a module the reader has since
//! deleted.

use moon_core::config::layout::WindowLayout;
use moon_core::config::{ChartLabelRow, ChartLabelsCfg, ChartTabKind};

use crate::persistence::chart_persist::ChartTabSpec;

/// Whether this caption set already places the column, hidden parts included.
fn holds_filters(cfg: &ChartLabelsCfg) -> bool {
    cfg.rows.iter().any(ChartLabelRow::holds_strategy_filters)
}

/// Append the shipped row when it is missing.
///
/// Returns `Some(true)` when a row was added, `Some(false)` when the set already placed one, and
/// `None` when there is no free module.
fn append_if_missing(cfg: &mut ChartLabelsCfg) -> Option<bool> {
    if holds_filters(cfg) {
        return Some(false);
    }
    if cfg
        .push_prepared(moon_core::config::strategy_filters_row())
        .is_some()
    {
        cfg.sanitize();
        Some(true)
    } else {
        None
    }
}

/// Live kinds that receive the module. Compare and Trade keep the sets they ship.
fn kind_receives(kind: ChartTabKind) -> bool {
    matches!(kind, ChartTabKind::Main | ChartTabKind::AddTo)
}

/// What one pass changed, so the caller writes only the files it has to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Carried {
    /// Whether any tab spec was rewritten, and `charts.json` therefore has to be saved.
    pub specs_changed: bool,
}

/// Insert the strategy-filter column into stored live caption sets that do not already place one.
///
/// Args:
///     layout: The loaded window layout — its global captions and its per-kind defaults.
///     specs: Every loaded chart tab spec; only a tab with its own caption override is rewritten.
///
/// Returns:
///     What the pass did, or `None` when it had already run.
pub(super) fn migrate_strategy_filters(
    layout: &mut WindowLayout,
    specs: &mut [ChartTabSpec],
) -> Option<Carried> {
    if layout.chart_strategy_filters_migrated {
        return None;
    }
    let mut carried = Carried::default();
    match append_if_missing(&mut layout.chart_labels) {
        Some(_) => {}
        None => {
            log::warn!(
                "нет свободного модуля под фильтры стратегий в подписях по умолчанию — перенос пропущен"
            );
        }
    }
    if let Some(mut cfg) = layout.stored_chart_labels(ChartTabKind::AddTo).cloned() {
        if append_if_missing(&mut cfg) == Some(true) {
            layout.store_chart_labels(ChartTabKind::AddTo, cfg);
        }
    }
    for spec in specs.iter_mut() {
        let kind = crate::chart_tabs::apply_all::spec_kind(spec);
        if !kind_receives(kind) {
            continue;
        }
        let Some(mut cfg) = spec.chart_labels.clone() else {
            continue;
        };
        match append_if_missing(&mut cfg) {
            Some(true) => {
                spec.chart_labels = Some(cfg);
                carried.specs_changed = true;
            }
            Some(false) => {}
            None => {
                log::warn!(
                    "вкладка {}/{}: нет свободного модуля под фильтры стратегий — перенос пропущен",
                    spec.group,
                    spec.num
                );
            }
        }
    }
    Some(carried)
}

#[cfg(test)]
mod tests;
