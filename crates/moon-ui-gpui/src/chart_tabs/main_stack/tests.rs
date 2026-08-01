//! Regression coverage for Main-chart Escape state transitions.

// NOT `use super::*`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use super::{remap_active_index, take_active_close_index};

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
