//! Pure selection arithmetic for the strategy list: the rows the list actually drew, and the
//! Shift-click range over them.
//!
//! Free of GPUI and of `AnalyticsView` so both halves are testable on their own — the click site
//! and the state transition live in `table` and `tuner::mod`, which own the context.

use moon_core::db::analytics::GroupStat;

use super::MAX_ROWS;
use crate::analytics::summary::strat_display;

/// What a click on a strategy row means, once its modifiers are read.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(in crate::analytics::tuner) enum RowClick {
    /// Double-click: open a Report scoped to this strategy.
    OpenReport,
    /// Plain click: single-select, or toggle the sole selection off.
    Single,
    /// Ctrl/Command: add or remove this row from the extras.
    Multi,
    /// Shift: hold everything between the anchor and this row.
    Range,
}

/// Resolve a row click's meaning from its native count and modifiers.
///
/// Double-click wins before every selection modifier. For single clicks, Shift wins over
/// Ctrl/Command, matching the strategies tree.
///
/// Args:
///     click_count: Native click count reported by GPUI.
///     shift: Whether Shift was held.
///     secondary: Whether the platform multi-select modifier was held (Ctrl, or ⌘ on macOS).
///
/// Returns:
///     The selection gesture to run.
pub(in crate::analytics::tuner) fn row_click_intent(
    click_count: usize,
    shift: bool,
    secondary: bool,
) -> RowClick {
    if click_count >= 2 {
        RowClick::OpenReport
    } else if shift {
        RowClick::Range
    } else if secondary {
        RowClick::Multi
    } else {
        RowClick::Single
    }
}

/// Convert Analytics `[from, to)` bounds to Report's inclusive optional bounds.
///
/// Args:
///     from: Inclusive Analytics lower bound, or a negative value for unbounded history.
///     to: Exclusive Analytics upper bound.
///
/// Returns:
///     Optional inclusive Report lower and upper bounds.
pub(in crate::analytics::tuner) fn inclusive_report_bounds(
    from: i64,
    to: i64,
) -> (Option<i64>, Option<i64>) {
    (
        (from >= 0).then_some(from),
        (to > 0).then_some(to.saturating_sub(1)),
    )
}

/// The rows the list actually DREW, as `(key, display name)` in display order.
///
/// Capped at [`MAX_ROWS`] because the virtual list is built with `total.min(MAX_ROWS)` rows and
/// never asks its factory for one past that. A range spanning the untruncated order could put
/// rows into the selection that were never rendered — write targets the user can neither see nor
/// untick.
///
/// Names go through `strat_display` exactly as the row renderer does, so a range-built selection
/// stores the same label a Ctrl-click would (`strategyid = 0` reads as "manual orders", not "0").
///
/// Args:
///     all: The complete group set the visible order indexes into.
///     visible: Row indices in display order, as `visible_indices` returns them.
///
/// Returns:
///     Key and display name of every drawn row, in display order.
pub(in crate::analytics::tuner) fn drawn_order(
    all: &[GroupStat],
    visible: &[usize],
) -> Vec<(String, String)> {
    visible
        .iter()
        .take(MAX_ROWS)
        .filter_map(|i| all.get(*i))
        .map(|g| (g.key.clone(), strat_display(&g.name)))
        .collect()
}

/// What a Shift-click resolves to against the drawn order.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::analytics::tuner) enum RangeOutcome {
    /// Hold exactly these extras.
    Extras(Vec<(String, String)>),
    /// No range is spannable, but the click still means something: select the clicked row.
    SingleSelect,
    /// The drawn order is not resolvable right now — do nothing at all.
    Ignore,
}

/// Extras for a Shift-click: every drawn row between the anchor and the clicked row, in display
/// order, with the anchor itself excluded.
///
/// Display order is load-bearing rather than cosmetic: Ctrl-removing the anchor promotes
/// `sel_extra.remove(0)` into it, so the promoted strategy must be the topmost row of the span
/// and not whichever one a set happened to yield.
///
/// Three outcomes rather than an `Option`, because "no range" has two very different causes:
///
/// * `SingleSelect` — there is no anchor at all, or the order IS known and simply does not hold
///   the anchor or the clicked row (filtered out by the search bar, or past [`MAX_ROWS`]).
///   Falling back to a plain select is right: the row the user clicked must not be lost to a
///   modifier that had nothing to span.
/// * `Ignore` — the order is EMPTY, which cannot happen while a row is on screen to be clicked.
///   It means the memoized order was dropped when the group set was replaced and the frame that
///   rebuilds it has not run yet, so the click is being resolved against a list that no longer
///   describes what the user saw. Falling back to a single select there would clear the whole
///   multi-selection, move the anchor and reset the By-time schedule grid — destroying exactly
///   what the gesture was extending. A repaint is already scheduled; doing nothing is correct.
///
/// Args:
///     anchor_key: Row key of the current anchor, or `None` when nothing is selected yet.
///     clicked_key: Row key the user shift-clicked.
///     order: Drawn rows in display order, from [`drawn_order`].
///
/// Returns:
///     The extras to hold, or why no range could be taken.
pub(in crate::analytics::tuner) fn range_extras(
    anchor_key: Option<&str>,
    clicked_key: &str,
    order: &[(String, String)],
) -> RangeOutcome {
    // No anchor is answered before the emptiness check: there is no selection to protect, so the
    // click simply becomes the first one.
    let Some(anchor_key) = anchor_key else {
        return RangeOutcome::SingleSelect;
    };
    if order.is_empty() {
        return RangeOutcome::Ignore;
    }
    let ia = order.iter().position(|(k, _)| k == anchor_key);
    let ib = order.iter().position(|(k, _)| k == clicked_key);
    let (Some(ia), Some(ib)) = (ia, ib) else {
        return RangeOutcome::SingleSelect;
    };
    let (lo, hi) = if ia <= ib { (ia, ib) } else { (ib, ia) };
    RangeOutcome::Extras(
        order[lo..=hi]
            .iter()
            .filter(|(k, _)| k != anchor_key)
            .cloned()
            .collect(),
    )
}

#[cfg(test)]
mod tests;
