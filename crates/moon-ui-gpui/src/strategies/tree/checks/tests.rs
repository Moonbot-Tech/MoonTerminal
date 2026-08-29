//! Unit tests for the bulk-check switch: the order its click writes, and the press guard
//! that keeps the click off the row.
//!
//! Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
//! shadows the built-in attribute and makes `#[test]` expand recursively.

use super::stage_value;

/// Source of the module under test, for the guards a unit test cannot reach through `Context`.
const SRC: &str = include_str!("../checks.rs");

/// Returns the body of one function in [`SRC`], for the two order guards below.
fn body_of(signature: &str) -> &'static str {
    SRC.split(signature)
        .nth(1)
        .unwrap_or_else(|| panic!("{signature} must exist"))
}

/// Losing the workspace guard would let a stale callback stage a hidden Auto core in bulk — the
/// same defect `tree/moon/tests.rs` pins for the per-strategy checkbox, at a much larger blast
/// radius.
#[test]
fn the_workspace_guard_runs_before_any_bulk_mutation() {
    let body = body_of("pub(super) fn toggle_row_check(");
    let guard = body
        .find("strategy_core_is_visible(self.workspace_cores.as_deref(), core)")
        .expect("the bulk toggle must validate the current workspace");
    let staged_write = body
        .find("self.stage_check(")
        .expect("the bulk toggle must stage the rows it covers");

    assert!(guard < staged_write);
}

/// Storing a value equal to the core's own flag would inflate the footer's change count with rows
/// that ask the core for nothing, and a bulk click can inflate it by a whole account at once.
#[test]
fn a_value_equal_to_the_core_is_dropped_rather_than_staged() {
    assert_eq!(stage_value(true, false), Some(true));
    assert_eq!(stage_value(false, true), Some(false));
    assert_eq!(stage_value(true, true), None);
    assert_eq!(stage_value(false, false), None);
}

/// The press must be swallowed before it reaches the row, or one click on the box also collapses
/// the folder under it and a drifting press drags that folder away.
#[test]
fn the_checkbox_swallows_the_press_the_row_would_have_taken() {
    let body = body_of("pub(super) fn bulk_check(");
    let stop = body
        .find("app.stop_propagation()")
        .expect("the wrapper must stop the press");
    let child = body
        .find(".child(")
        .expect("the wrapper must have the checkbox as a child");
    let checkbox = body
        .find("row_checkbox(")
        .expect("the wrapper must contain the checkbox");

    assert!(
        stop < child && child < checkbox,
        "the press guard must sit on the container the checkbox is a child of"
    );
}
