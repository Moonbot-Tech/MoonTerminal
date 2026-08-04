//! Regression coverage for Main-chart Escape state transitions.

// NOT `use super::*`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use super::{remap_active_index, stack_toggle_target, take_active_close_index};

/// Reintroducing a tiled-mode early return in `main_stack.rs:take_active_close_index`, or clearing
/// `active` while charts remain, must fail: Escape would work only once or only in fullscreen.
#[test]
fn escape_closes_repeatedly_in_fullscreen_and_stack_modes() {
    for initial_stack_mode in [false, true] {
        let mut active = Some(1);
        let mut show_stack = initial_stack_mode;

        assert_eq!(
            take_active_close_index(&mut active, &mut show_stack, 3),
            Some(1)
        );
        assert_eq!(active, Some(1));
        assert!(show_stack);

        assert_eq!(
            take_active_close_index(&mut active, &mut show_stack, 2),
            Some(1)
        );
        assert_eq!(active, Some(0));
        assert!(show_stack);

        assert_eq!(
            take_active_close_index(&mut active, &mut show_stack, 1),
            Some(0)
        );
        assert_eq!(active, None);
        assert!(!show_stack);
    }
}

/// Removing the fallback in `main_stack.rs:take_active_close_index` must fail: stack presentation
/// can legitimately have no selected tile, and Escape must still close the first visible chart.
#[test]
fn escape_falls_back_to_the_first_chart_when_stack_has_no_active_index() {
    let mut active = None;
    let mut show_stack = true;

    assert_eq!(
        take_active_close_index(&mut active, &mut show_stack, 2),
        Some(0)
    );
    assert_eq!(active, Some(0));
    assert!(show_stack);
}

/// Returning the previous numeric index from `main_stack.rs:remap_active_index` must fail: moving
/// a comparison anchor ahead of the active chart would make Escape close a different market.
#[test]
fn comparison_reordering_preserves_the_active_chart_identity() {
    let reordered = ["active", "anchor", "other"];

    let active = remap_active_index(reordered.len(), Some(&"active"), Some(1), |ix, key| {
        reordered[ix] == *key
    });

    assert_eq!(active, Some(0));
}

/// `main_stack.rs:stack_toggle_target` must leave a single chart alone.
///
/// Breakage this pins: restoring the unconditional `show_stack = !show_stack`. With one chart the
/// stack and fullscreen presentations draw the same content, so the flip changed nothing except to
/// add the tiled layout's gutter — the chart jumped up by that strip and back on every
/// right-click, which reads as the chart twitching rather than as a mode change.
#[test]
fn right_click_never_expands_a_single_chart_into_a_stack() {
    assert_eq!(
        stack_toggle_target(false, 1),
        None,
        "a lone fullscreen chart has no stack to expand into"
    );
    assert_eq!(
        stack_toggle_target(false, 0),
        None,
        "an empty stack has nothing to toggle"
    );
    assert_eq!(
        stack_toggle_target(true, 1),
        Some(false),
        "returning to fullscreen must stay available: charts expire and can leave a stack of one"
    );
    assert_eq!(
        stack_toggle_target(false, 2),
        Some(true),
        "with siblings the gesture works as before"
    );
    assert_eq!(stack_toggle_target(true, 3), Some(false));
}
