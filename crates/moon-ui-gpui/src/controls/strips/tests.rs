//! Regression coverage for preset-strip wheel gating and handler wiring.

// NOT `use super::*`: the glob would pull in the `gpui::test` macro, and `#[test]` would
// expand into itself (recursion limit).
use super::{
    SellClickAction, SizeClickAction, sell_click_action, size_click_action, wheel_step_dir,
};
use gpui::{Modifiers, Point, ScrollDelta};

/// Build a vertical line-based scroll delta for wheel-gate tests.
///
/// Args:
///     y: Signed vertical line delta.
///
/// Returns:
///     A vertical `ScrollDelta`.
fn lines(y: f32) -> ScrollDelta {
    ScrollDelta::Lines(Point { x: 0.0, y })
}

/// Build modifiers containing only Ctrl.
///
/// Returns:
///     Ctrl-only modifiers.
fn ctrl() -> Modifiers {
    Modifiers {
        control: true,
        ..Modifiers::default()
    }
}

/// `strips.rs:wheel_step_dir` must reject an unmodified wheel gesture.
///
/// Removing the modifier gate would let ordinary toolbar scrolling silently rewrite an order size
/// or sell percentage.
#[test]
fn bare_wheel_never_changes_a_preset() {
    assert_eq!(wheel_step_dir(Modifiers::default(), lines(1.0)), None);
    assert_eq!(wheel_step_dir(Modifiers::default(), lines(-1.0)), None);
}

/// `strips.rs:wheel_step_dir` must preserve both Ctrl-wheel directions.
///
/// Reversing the Y comparison or rejecting Ctrl-modified input changes a trading value in the
/// wrong direction or leaves it unchanged.
#[test]
fn ctrl_wheel_reports_direction() {
    assert_eq!(wheel_step_dir(ctrl(), lines(1.0)), Some(true));
    assert_eq!(wheel_step_dir(ctrl(), lines(-1.0)), Some(false));
}

/// `strips.rs:wheel_step_dir` must reject a horizontal trackpad gesture.
///
/// Treating zero Y as a downward step would shrink a trading parameter during sideways scrolling.
#[test]
fn horizontal_gesture_is_not_a_downward_step() {
    assert_eq!(wheel_step_dir(ctrl(), lines(0.0)), None);
}

/// `strips.rs:size_click_action` must preserve selection and double-click editing with the native
/// clicked index. Reversing or collapsing the click-count branch selects a preset when the user
/// requested its inline editor.
#[test]
fn order_size_clicks_preserve_selection_and_edit_semantics() {
    assert_eq!(size_click_action(4, 1), SizeClickAction::Select(4));
    assert_eq!(size_click_action(2, 2), SizeClickAction::Edit(2));
    assert_eq!(size_click_action(5, 3), SizeClickAction::Edit(5));
}

/// `strips.rs:sell_click_action` must distinguish fixed-slot selection, active-slot restoration,
/// and double-click editing. Treating the selected slot as zero-based or evaluating it before the
/// double-click branch changes the live take-profit mode instead of opening the editor.
#[test]
fn fixed_sell_clicks_preserve_slot_and_edit_semantics() {
    assert_eq!(
        sell_click_action(3, Some(1), 1),
        SellClickAction::SelectFixed(4)
    );
    assert_eq!(
        sell_click_action(3, Some(4), 1),
        SellClickAction::EngageMain
    );
    assert_eq!(sell_click_action(3, Some(4), 2), SellClickAction::Edit(3));
}

/// Both `strips.rs:size_strip` and `sell_strip` must route native MoonUI scroll callbacks through
/// the Ctrl gate.
///
/// Reading the raw delta in either callback would defeat the gate while leaving the pure helper
/// tests green.
#[test]
fn both_wheel_handlers_consult_the_gate_rather_than_the_raw_delta() {
    let source = include_str!("../strips.rs");
    let implementation = source.split("#[cfg(test)]").next().unwrap_or(source);

    assert_eq!(
        implementation
            .matches("wheel_step_dir(event.modifiers, event.delta)")
            .count(),
        2,
        "both preset strips must route native wheel events through wheel_step_dir"
    );
}
