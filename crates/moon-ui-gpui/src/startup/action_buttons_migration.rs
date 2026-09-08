//! One-shot carry-over of the chart's market BUTTONS into the caption configuration.
//!
//! `Cancel Buy` and `Panic Sell` used to be a per-tab setting of their own — `Hide`, `Left`,
//! `Centre` or `Right`, chosen in the ⚙ layout popup and stored as `cancel_buy_pos` /
//! `panic_sell_pos` in `charts.json`. They are captions now, placed like everything else the chart
//! prints, so those two keys have nowhere to be read from and every tab would silently land on the
//! shipped pair — bottom right — whatever the reader had chosen.
//!
//! What the pass does, in the terms the old setting expressed:
//!
//! - `Hide` writes no row at all: the button was not drawn, and it stays undrawn.
//! - `Left` / `Centre` / `Right` become one row each in the plot's BOTTOM band, aligned to that
//!   side. That is where the buttons were drawn — along the bottom edge, above the time axis.
//! - Two buttons on the SAME side keep their line and their order, because that is what the old
//!   layout did with two buttons sharing an anchor.
//!
//! Where the rows go depends on what the tab already had. A tab with its own caption set gets them
//! appended to it. A tab with none, but with a position that differs from the shipped pair, is
//! given an override built from the default it was following — the alternative is losing the
//! choice, and this is the only per-tab home a caption has.
//!
//! Guarded by `WindowLayout::chart_action_buttons_migrated` and, like the graphics migration beside
//! it, it neither sets that marker nor saves anything: both are the caller's, because the marker
//! may only be committed once the tab specs are known to be on disk.

use moon_core::config::layout::WindowLayout;
use moon_core::config::{ChartLabelRow, ChartLabelsCfg, LabelAlign};

use crate::persistence::chart_persist::{ChartBtnPos, ChartTabSpec};

/// The shipped pair as the old setting spelled it: both buttons pushed right.
///
/// A tab holding exactly this asked for nothing the default does not already give, which is what
/// lets the pass leave such a tab alone rather than freezing its captions into an override.
const DEFAULT_POS: (ChartBtnPos, ChartBtnPos) = (ChartBtnPos::Right, ChartBtnPos::Right);

/// Where in its band a button sat, or `None` for one that was hidden.
fn align_of(pos: ChartBtnPos) -> Option<LabelAlign> {
    match pos {
        ChartBtnPos::Hide => None,
        ChartBtnPos::Left => Some(LabelAlign::Left),
        ChartBtnPos::Center => Some(LabelAlign::Center),
        ChartBtnPos::Right => Some(LabelAlign::Right),
    }
}

/// The rows one tab's two positions become, through the model's own builder.
///
/// The ordering rule and the band live in `moon_core` beside the shipped set — see
/// [`moon_core::config::action_rows_at`] — so a migrated chart and a fresh one cannot end up
/// mirroring each other. All this adds is the old setting's vocabulary: `Hide` means "not drawn".
fn rows_for(cancel: ChartBtnPos, panic: ChartBtnPos) -> Vec<ChartLabelRow> {
    moon_core::config::action_rows_at(align_of(cancel), align_of(panic))
}

/// Whether this caption set already places a button, and so must not be given another.
fn holds_a_button(cfg: &ChartLabelsCfg) -> bool {
    cfg.rows.iter().any(ChartLabelRow::holds_action)
}

/// Append rows to a caption set, reporting whether anything was added.
///
/// Out of room is not a failure: the reader is already at the sixteen-module ceiling, and dropping
/// a module of theirs to fit a button in would be the worse trade.
fn append(cfg: &mut ChartLabelsCfg, rows: Vec<ChartLabelRow>) -> bool {
    let mut added = false;
    for row in rows {
        if cfg.push_prepared(row).is_some() {
            added = true;
        }
    }
    if added {
        cfg.sanitize();
    }
    added
}

/// Give a stored DEFAULT the shipped pair, unless it already places a button.
///
/// The guard is what makes a repeated pass harmless: a default that already holds a button has
/// been through this, or the reader put one there themselves, and a second pair would sit under
/// the first.
fn append_default(cfg: &mut ChartLabelsCfg, rows: Vec<ChartLabelRow>) -> bool {
    !holds_a_button(cfg) && append(cfg, rows)
}

/// Replace whatever buttons a caption set inherited with the ones this tab actually drew.
///
/// The stripping is the point. A tab with no set of its own is built from the DEFAULT it was
/// following, and that default now carries the shipped pair — bottom right. Keeping those and
/// adding the tab's own would draw four buttons, two of them where the reader had moved them from.
fn set_buttons(cfg: &mut ChartLabelsCfg, rows: Vec<ChartLabelRow>) -> bool {
    for row in cfg.rows.iter_mut() {
        if row.holds_action() {
            *row = ChartLabelRow::default();
        }
    }
    // Sanitized before the append, not only after: clearing a row leaves a hole, and a hole is what
    // `push_prepared` would otherwise write the first button into — putting it in the middle of the
    // reader's modules rather than after them.
    cfg.sanitize();
    // An empty list is a faithful result, not a failure: `Hide` on both buttons asked for exactly
    // this, and the stripping above is what carries that choice over.
    rows.is_empty() || append(cfg, rows)
}

/// What one pass changed, so the caller writes only the files it has to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Carried {
    /// Whether any tab spec was rewritten, and `charts.json` therefore has to be saved. False on a
    /// fresh profile and on one whose tabs all sat at the shipped position — there is nothing in
    /// them to record, and rewriting the file to say so is a write nobody asked for.
    pub specs_changed: bool,
}

/// Carry every stored button position into the captions that now draw them.
///
/// The TRADE window's own default is deliberately left alone: it draws a trade that already closed
/// and prints no button at any position, so a row there would be a module that can never appear.
///
/// Args:
///     layout: The loaded window layout — its global captions and its per-kind defaults.
///     specs: Every loaded chart tab spec; the legacy keys are cleared as each is carried over.
///
/// Returns:
///     What the pass did, or `None` when it had already run.
pub(super) fn migrate_action_buttons(
    layout: &mut WindowLayout,
    specs: &mut [ChartTabSpec],
) -> Option<Carried> {
    if layout.chart_action_buttons_migrated {
        return None;
    }
    let mut carried = Carried::default();
    // The shipped pair, which is what every stored default was drawing: the old positions lived per
    // TAB, so a default could only ever mean "wherever each tab says", and an untouched tab said
    // bottom right.
    let default_rows = || rows_for(DEFAULT_POS.0, DEFAULT_POS.1);
    let mut global = layout.chart_labels.clone();
    if holds_a_button(&global) {
        // Already carried over, or the reader placed one themselves. Nothing to do, and nothing to
        // warn about.
    } else if append_default(&mut global, default_rows()) {
        layout.chart_labels = global;
    } else {
        log::warn!(
            "нет свободного модуля под кнопки чарта в подписях по умолчанию — перенос пропущен"
        );
    }
    for kind in [
        moon_core::config::ChartTabKind::AddTo,
        moon_core::config::ChartTabKind::Compare,
    ] {
        // Only a kind that HOLDS a set of its own: an empty slot follows Main or its own shipped
        // set, and both of those already carry the pair.
        let Some(mut cfg) = layout.stored_chart_labels(kind).cloned() else {
            continue;
        };
        if append_default(&mut cfg, default_rows()) {
            layout.store_chart_labels(kind, cfg);
        }
    }
    for spec in specs.iter_mut() {
        // An absent key meant the shipped position, not "no button" — `ChartBtnPos::default()` is
        // `Right` — so a tab that stated only one of the two keeps the default for the other.
        let stated = spec.cancel_buy_pos.is_some() || spec.panic_sell_pos.is_some();
        let cancel = spec.cancel_buy_pos.take().unwrap_or_default();
        let panic = spec.panic_sell_pos.take().unwrap_or_default();
        // Nothing stated, or exactly what the shipped pair already draws: the tab keeps following
        // whatever it followed, and is not frozen into an override for a choice it never made.
        if !stated || (cancel, panic) == DEFAULT_POS {
            continue;
        }
        let mut cfg = match spec.chart_labels.clone() {
            Some(cfg) => cfg,
            // No override of its own: build one from the set it was following, so the tab keeps
            // every caption it had and gains only the buttons where it put them. Which set that is
            // comes from the SHARED rule for a stored tab — a docked numbered tab follows Main —
            // rather than from a second reading of the same three fields.
            None => layout
                .chart_labels_for(crate::chart_tabs::apply_all::spec_kind(spec))
                .clone(),
        };
        if !set_buttons(&mut cfg, rows_for(cancel, panic)) {
            // At the sixteen-module ceiling. Said out loud rather than swallowed: the tab keeps
            // every module it has, and the reader is told which button they now have to place by
            // hand instead of finding it missing and having to guess why.
            log::warn!(
                "вкладка {}/{}: нет свободного модуля под кнопки чарта — перенос пропущен",
                spec.group,
                spec.num
            );
            continue;
        }
        spec.chart_labels = Some(cfg);
        carried.specs_changed = true;
    }
    Some(carried)
}

#[cfg(test)]
mod tests;
