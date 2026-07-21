// NOT `use super::*`: the glob would pull in the `gpui::test` macro, and `#[test]` would
// expand into itself (recursion limit).
use super::{cell_width, wheel_step_dir};
use gpui::{Modifiers, Point, ScrollDelta};

fn lines(y: f32) -> ScrollDelta {
    ScrollDelta::Lines(Point { x: 0.0, y })
}

fn ctrl() -> Modifiers {
    Modifiers {
        control: true,
        ..Modifiers::default()
    }
}

#[test]
fn bare_wheel_never_changes_a_preset() {
    // Removing the modifier gate would let scrolling over the toolbar silently rewrite the
    // order size in the config and the sell percentage in the core.
    assert_eq!(wheel_step_dir(Modifiers::default(), lines(1.0)), None);
    assert_eq!(wheel_step_dir(Modifiers::default(), lines(-1.0)), None);
}

#[test]
fn ctrl_wheel_reports_direction() {
    // Reversing the Y comparison or rejecting Ctrl-modified input makes one of these assertions
    // fail before a preset is adjusted in the wrong direction or not adjusted at all.
    assert_eq!(wheel_step_dir(ctrl(), lines(1.0)), Some(true));
    assert_eq!(wheel_step_dir(ctrl(), lines(-1.0)), Some(false));
}

#[test]
fn horizontal_gesture_is_not_a_downward_step() {
    // ScrollDelta is two-dimensional: a sideways gesture carries y == 0. A naive `y > 0.0`
    // would return "down" and SHRINK a trading parameter from horizontal scrolling.
    assert_eq!(wheel_step_dir(ctrl(), lines(0.0)), None);
}

/// Regression: dropping cell padding from `cell_width` squeezes the rendered preset label.
#[test]
fn a_cell_leaves_room_for_its_own_label() {
    // Plausible future edit: `controls::strips::cell_width` loses its `pad` term — someone
    // decides the measured text width is enough and drops the cell's own padding
    // (`CELL_PAD_X` + `CELL_HOTKEY_GAP`) as slack. Visible consequence: the label is squeezed
    // inside a box that was never sized for it again — exactly the crowding at a larger font
    // that made the width content-measured in the first place.
    //
    // The oracle is independent of the code: the test supplies both quantities, and "a cell is
    // never narrower than its content plus its padding" comes from the contract, not from the
    // implementation.
    let text = 40.0;
    let pad = 27.0;
    assert!(
        cell_width(text, pad, 34.0) >= text + pad,
        "a cell must fit its label together with its own padding"
    );
}

/// Regression: removing the minimum width makes short preset cells difficult to click.
#[test]
fn a_short_label_still_gets_a_clickable_cell() {
    // The floor is a mouse target: "1%" is narrower than the padding on its own, and without
    // the clamp the cell would collapse into a slit that is awkward to hit.
    assert_eq!(cell_width(6.0, 4.0, 34.0), 34.0);
}

/// Regression: removing pixel rounding can desynchronize cells from their hit targets.
#[test]
fn a_cell_width_is_whole_pixels() {
    // A second plausible edit to the same function, likelier than losing `pad`: `.ceil()` looks
    // like cosmetic rounding and gets removed as redundant. But a fractional width is precisely
    // the source of rounding divergence between the strip and its interaction layer that the
    // layer was rewritten as a flex row to avoid (see `strip_with_overlay`) — GPUI rounds every
    // length to a device pixel separately.
    //
    // The input is fractional ON PURPOSE: on the whole-number inputs of the two tests above,
    // losing `.ceil()` would go unnoticed.
    assert_eq!(cell_width(40.5, 26.0, 34.0), 67.0);
}

#[test]
fn wheel_handler_consults_the_gate_rather_than_the_raw_delta() {
    // Guards the CALL SITE, which the three tests above cannot reach: they all exercise the
    // pure `wheel_step_dir` helper, so reading the raw delta directly in `on_scroll_wheel`
    // would defeat the Ctrl gate entirely and leave every one of them green. The gate is only
    // effective if the handler actually calls it.
    let source = include_str!("../strips.rs");
    let implementation = source.split("#[cfg(test)]").next().unwrap_or(source);
    let handler = implementation
        .split(".on_scroll_wheel(")
        .nth(1)
        .expect("the strip overlay must still install a scroll-wheel handler");

    assert!(
        handler.contains("wheel_step_dir(ev.modifiers, ev.delta)"),
        "the wheel handler must route through wheel_step_dir, not read the delta directly"
    );
}
