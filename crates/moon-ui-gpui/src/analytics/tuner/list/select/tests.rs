//! Unit tests for the strategy list's Shift-click range.
//!
//! Explicit imports rather than `use super::*`: the parent chain re-exports `gpui::*`, whose own
//! `test` shadows the built-in attribute and makes `#[test]` expand recursively.

use moon_core::db::analytics::GroupStat;

use super::super::MAX_ROWS;
use super::{RangeOutcome, RowClick, drawn_order, range_extras, row_click_intent};

/// A group carrying only the two fields the range reads.
fn g(name: &str) -> GroupStat {
    GroupStat {
        key: format!("{name}@1"),
        name: name.to_string(),
        ..GroupStat::default()
    }
}

/// The extras an outcome holds, failing loudly on any other outcome.
fn extras_of(outcome: RangeOutcome) -> Vec<(String, String)> {
    match outcome {
        RangeOutcome::Extras(extras) => extras,
        other => panic!("expected a spanned range, got {other:?}"),
    }
}

/// Five rows, drawn in the order given.
fn rows() -> (Vec<GroupStat>, Vec<usize>) {
    let all: Vec<GroupStat> = ["a", "b", "c", "d", "e"].iter().map(|n| g(n)).collect();
    let visible = (0..all.len()).collect();
    (all, visible)
}

/// The span runs from the anchor to the clicked row and holds everything between — except the
/// anchor, which is `sel_strategy` and is not an extra.
///
/// Breakage this pins: including the anchor in the returned extras. `selected_targets` puts the
/// anchor first and then appends the extras, so the save dialog would list — and write — the same
/// strategy twice.
#[test]
fn a_shift_range_spans_the_drawn_order_and_excludes_the_anchor() {
    let (all, visible) = rows();
    let order = drawn_order(&all, &visible);

    let extras = extras_of(range_extras(Some("b@1"), "e@1", &order));

    assert_eq!(
        extras.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        vec!["c@1", "d@1", "e@1"],
        "the anchor stays out of the extras, the rest of the span is in"
    );
}

/// Clicking ABOVE the anchor selects the same block as clicking below it.
///
/// Breakage this pins: dropping the `lo`/`hi` swap. `order[lo..=hi]` with `lo > hi` would panic,
/// or — if written as a guard that returns empty — an upward shift-click would select nothing,
/// which reads as a dead modifier.
#[test]
fn a_backwards_range_selects_the_same_rows() {
    let (all, visible) = rows();
    let order = drawn_order(&all, &visible);

    let down = extras_of(range_extras(Some("b@1"), "d@1", &order));
    let up = extras_of(range_extras(Some("d@1"), "b@1", &order));

    assert_eq!(
        down.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        vec!["c@1", "d@1"]
    );
    // Anchored at "d", so "d" is the excluded one and "b" joins the extras.
    assert_eq!(
        up.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        vec!["b@1", "c@1"]
    );
}

/// The extras keep DISPLAY order, because position 0 of that list is load-bearing.
///
/// Breakage this pins: reversing the span, or collecting it through an unordered container. The
/// whole sequence is asserted, not merely its head — a single-element check could still pass by
/// chance under a container that happens to yield the top row first. Ctrl-removing the anchor
/// promotes `sel_extra.remove(0)` into it, so a mis-ordered span would re-scope the whole page to
/// a strategy other than the top of the block the user selected.
#[test]
fn the_range_keeps_display_order_so_anchor_removal_promotes_the_topmost_row() {
    let (all, visible) = rows();
    let order = drawn_order(&all, &visible);

    let extras = extras_of(range_extras(Some("d@1"), "a@1", &order));

    assert_eq!(
        extras.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        vec!["a@1", "b@1", "c@1"],
        "the span must arrive in display order, top row first"
    );
}

/// Shift wins over Ctrl/Command when both are held.
///
/// Breakage this pins: writing the click site as `if secondary { .. } else if shift { .. }`.
/// Shift-clicking with Ctrl still pressed — which is exactly what happens when a user extends a
/// selection they just built with Ctrl — would then toggle a single row instead of taking the
/// range, silently discarding the gesture.
#[test]
fn shift_takes_precedence_over_the_multi_select_modifier() {
    assert_eq!(row_click_intent(true, true), RowClick::Range);
    assert_eq!(row_click_intent(true, false), RowClick::Range);
    assert_eq!(row_click_intent(false, true), RowClick::Multi);
    // A keyboard-activated click reports no modifiers at all, so it lands here.
    assert_eq!(row_click_intent(false, false), RowClick::Single);
}

/// An anchor the list is not drawing yields no range at all.
///
/// Breakage this pins: replacing `position()?` with `unwrap_or(0)`. After typing in the search box
/// the anchor can be filtered out, and the range would then silently span from the top row of the
/// list — a block the user never anchored.
#[test]
fn a_filtered_out_anchor_yields_no_range() {
    let (all, visible) = rows();
    let order = drawn_order(&all, &visible);

    assert_eq!(
        range_extras(Some("zz@1"), "c@1", &order),
        RangeOutcome::SingleSelect,
        "an anchor outside the drawn rows cannot start a range"
    );
    assert_eq!(
        range_extras(Some("a@1"), "zz@1", &order),
        RangeOutcome::SingleSelect,
        "a clicked row outside the drawn rows cannot end one either"
    );
}

/// The order stops where the list stops drawing.
///
/// Breakage this pins: dropping `.take(MAX_ROWS)`. The virtual list is built with
/// `total.min(MAX_ROWS)` rows, so a range over the untruncated order could put rows into the
/// selection that were never rendered — write targets the user can neither see nor untick.
#[test]
fn drawn_order_stops_at_the_rows_the_list_actually_drew() {
    let all: Vec<GroupStat> = (0..MAX_ROWS + 5).map(|i| g(&format!("s{i}"))).collect();
    let visible: Vec<usize> = (0..all.len()).collect();

    let order = drawn_order(&all, &visible);

    assert_eq!(order.len(), MAX_ROWS);
    assert_eq!(
        range_extras(Some("s0@1"), &format!("s{}@1", MAX_ROWS), &order),
        RangeOutcome::SingleSelect,
        "a row past the drawn window is not a valid range end"
    );
}

/// Rows are labelled exactly as the row renderer labels them.
///
/// Breakage this pins: using `g.name` raw. `strategyid = 0` means manual orders and renders as a
/// localized label, so a raw name would make the same strategy carry a different title depending
/// on whether it was Ctrl-clicked or shift-ranged — including in the save dialog's heading.
#[test]
fn drawn_order_labels_rows_exactly_as_the_row_renderer_does() {
    let all = vec![g("0"), g("alpha")];
    let visible = vec![0, 1];

    let order = drawn_order(&all, &visible);

    assert_eq!(
        order[0].1,
        crate::analytics::summary::strat_display("0"),
        "manual orders must read as the label, not as a bare id"
    );
    assert_ne!(order[0].1, "0", "the raw id must not reach the selection");
    assert_eq!(order[1].1, "alpha");
}

/// An empty drawn order means the list's order is momentarily unknown — never a single-select.
///
/// `analytics/mod.rs` drops the memoized order the instant the group set is replaced, in the DB
/// result callback, while the previous frame's rows are still hit-testable. A Shift-click landing
/// in that window resolves against an empty order.
///
/// Breakage this pins: collapsing `Ignore` into `SingleSelect` (the shape an `Option` return would
/// force). `select_single` on a non-anchor row clears every extra and calls `set_sel_strategy`,
/// which moves the anchor and resets the By-time schedule grid — so extending a selection would
/// occasionally destroy it instead, with nothing on screen to explain why.
#[test]
fn an_unresolvable_order_is_ignored_rather_than_collapsed_to_a_single_select() {
    assert_eq!(
        range_extras(Some("a@1"), "c@1", &[]),
        RangeOutcome::Ignore,
        "an empty order must not be answered with a destructive fallback"
    );
    // Contrast: a KNOWN order that simply lacks the rows is a genuine single-select.
    let (all, visible) = rows();
    let order = drawn_order(&all, &visible);
    assert_eq!(
        range_extras(Some("zz@1"), "yy@1", &order),
        RangeOutcome::SingleSelect,
        "a known order that lacks both rows still means the click selects one"
    );
}

/// With nothing selected yet, a Shift-click is simply the first selection.
///
/// This answer comes BEFORE the empty-order check on purpose: there is no selection to protect,
/// so the click must not be swallowed just because the order is momentarily unknown.
///
/// Breakage this pins: moving the no-anchor branch after the emptiness test. The very first
/// Shift-click on a freshly opened window — where the order can still be unbuilt — would then do
/// nothing at all, which reads as a dead list.
#[test]
fn a_shift_click_with_no_anchor_selects_the_clicked_row() {
    let (all, visible) = rows();
    let order = drawn_order(&all, &visible);

    assert_eq!(
        range_extras(None, "c@1", &order),
        RangeOutcome::SingleSelect
    );
    assert_eq!(
        range_extras(None, "c@1", &[]),
        RangeOutcome::SingleSelect,
        "no anchor outranks an unknown order — there is no selection to lose"
    );
}
