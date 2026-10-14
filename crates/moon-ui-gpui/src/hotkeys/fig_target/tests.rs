use std::cell::Cell;

use super::*;

/// What one `fig_delete` press did, recorded through the two callbacks.
#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    consumed: bool,
    deleted: Option<FigDeleteTarget<u64>>,
    cancel_tried: bool,
}

/// Run one press with a cancel that reports `order_under_cursor` as its result.
fn press(
    hovered: Option<u64>,
    selected: Option<u64>,
    in_reach: bool,
    order_under_cursor: bool,
) -> Outcome {
    let deleted = Cell::new(None);
    let cancel_tried = Cell::new(false);
    let consumed = press_fig_delete(
        hovered,
        selected,
        in_reach,
        &mut (),
        |t, _| deleted.set(Some(t)),
        |_| {
            cancel_tried.set(true);
            order_under_cursor
        },
    );
    Outcome {
        consumed,
        deleted: deleted.take(),
        cancel_tried: cancel_tried.get(),
    }
}

/// Branch a. Plausible breakage: trying the cancel before the hovered figure. Delete aimed at a
/// lit drawing would then cancel the live buy lying under it, the exact report of #971.
#[test]
fn a_hovered_figure_is_deleted_and_cancel_never_runs() {
    for (selected, order) in [(None, true), (Some(2), true), (Some(2), false)] {
        assert_eq!(
            press(Some(1), selected, true, order),
            Outcome {
                consumed: true,
                deleted: Some(FigDeleteTarget::Hovered(1)),
                cancel_tried: false,
            }
        );
    }
}

/// Branch b. Plausible breakage: putting the selection ahead of the cancel. A figure selected by
/// an incidental plain click would then swallow a Delete aimed at an entry order.
#[test]
fn b_order_under_cursor_beats_the_selected_figure() {
    assert_eq!(
        press(None, Some(2), true, true),
        Outcome {
            consumed: true,
            deleted: None,
            cancel_tried: true,
        }
    );
}

/// Branch c. Plausible breakage: dropping the selection fallback. The key would then do nothing
/// for a figure the user selected and pointed away from, as before #971's fix to its hint.
#[test]
fn c_selected_figure_is_deleted_when_no_order_is_aimed_at() {
    assert_eq!(
        press(None, Some(2), true, false),
        Outcome {
            consumed: true,
            deleted: Some(FigDeleteTarget::Selected(2)),
            cancel_tried: true,
        }
    );
}

/// Branch c, reach. Plausible breakage: dropping the reach check. A figure selected on another
/// chart would then be deleted while the user cannot see it.
#[test]
fn c_selection_off_the_hovered_chart_is_not_deleted() {
    assert_eq!(
        press(None, Some(2), false, false),
        Outcome {
            consumed: false,
            deleted: None,
            cancel_tried: true,
        }
    );
}

/// Branch d. Plausible breakage: treating "nothing aimed" as consumed. The key would then stop
/// propagating to whatever else it means in the window.
#[test]
fn d_nothing_aimed_is_not_consumed() {
    assert_eq!(
        press(None, None, true, false),
        Outcome {
            consumed: false,
            deleted: None,
            cancel_tried: true,
        }
    );
}

/// Plausible breakage: a plain click leaving the knots where they were. The key would then keep
/// pointing at the older figure the user had stopped working on.
#[test]
fn plain_click_moves_selection() {
    assert_eq!(
        plain_click_selection(Some(2), Some(&1), true),
        Some(Some(2))
    );
    assert_eq!(
        plain_click_selection(Some(2), Some(&1), false),
        Some(Some(2))
    );
    assert_eq!(plain_click_selection(Some(3), None, false), Some(Some(3)));
    assert_eq!(plain_click_selection(Some(2), Some(&2), true), None);
}

/// Plausible breakage: clearing a selection made on another chart. A click in one panel would
/// silently drop knots the user can still see in another window.
#[test]
fn empty_click_clears_only_this_charts_selection() {
    assert_eq!(
        plain_click_selection::<u64>(None, Some(&1), true),
        Some(None)
    );
    assert_eq!(plain_click_selection::<u64>(None, Some(&1), false), None);
    assert_eq!(plain_click_selection::<u64>(None, None, true), None);
}
