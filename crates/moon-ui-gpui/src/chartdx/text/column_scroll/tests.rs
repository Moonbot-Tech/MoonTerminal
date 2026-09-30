//! Unit checks for the label-column window and the wheel-notch arithmetic.
//!
//! Explicit imports: the chartdx parent re-exports `gpui::*`, whose own `test` shadows the
//! built-in attribute.

use moon_core::config::{ARB_MAX_ROWS, label_scroll_first_max};

use super::{clamp_first, notch_steps, visible_window};

const TOTALS: [usize; 7] = [0, 1, 31, 32, 33, 40, 100];

/// `column_scroll.rs:visible_window` dropping `room = room.saturating_sub(1);` (the room kept for
/// the "... N more" line) lets the window spend the whole budget on rows, so with the indicator
/// added the column is one line over the 32-line budget: the last row's part index spills into
/// the next column's range and the "... N more" line replaces or overlaps a row.
///
/// The oracle is arithmetic on the rows themselves: every hidden and drawn row is counted once,
/// the indicators are counted on top, and the total may never pass the budget.
#[test]
fn a_window_never_exceeds_the_budget_and_accounts_for_every_row() {
    for total in TOTALS {
        let max = label_scroll_first_max(total);
        let firsts = (0..=max as u32).chain([u32::MAX]);
        for first in firsts {
            let w = visible_window(total, first, ARB_MAX_ROWS);
            assert_eq!(
                w.above + w.range.len() + w.below,
                total,
                "rows lost or duplicated: total {total} first {first} -> {w:?}"
            );
            let lines = w.range.len() + usize::from(w.above > 0) + usize::from(w.below > 0);
            assert!(
                lines <= ARB_MAX_ROWS,
                "{lines} lines over the {ARB_MAX_ROWS}-line budget: total {total} first {first} -> {w:?}"
            );
            if total <= ARB_MAX_ROWS && first == 0 {
                assert_eq!(
                    (w.above, w.below),
                    (0, 0),
                    "a fitting column has no indicator"
                );
                assert_eq!(w.range, 0..total);
            }
        }
    }
}

/// `column_scroll.rs:visible_window` losing rows at a window edge (an off-by-one in `end` or
/// `room`) makes a row that no scroll position ever shows, so the reader cannot see it at all.
/// Every row of every column must be inside the window of some allowed first row.
#[test]
fn every_row_is_reachable_by_some_scroll_position() {
    for total in TOTALS {
        let mut seen = vec![false; total];
        for first in 0..=label_scroll_first_max(total) as u32 {
            for row in visible_window(total, first, ARB_MAX_ROWS).range {
                seen[row] = true;
            }
        }
        let missing: Vec<usize> = (0..total).filter(|row| !seen[*row]).collect();
        assert!(
            missing.is_empty(),
            "total {total}: rows never shown {missing:?}"
        );
    }
}

/// `column_scroll.rs:clamp_first` clamping against `total` instead of
/// `label_scroll_first_max(total)` lets a stale or repeated offset scroll the column past its
/// tail until it draws nothing but the "N above" line. Bounds are literals: the last allowed
/// first row keeps a two-row tail.
#[test]
fn clamp_first_keeps_the_tail_and_never_goes_negative() {
    assert_eq!(clamp_first(-5, 40), 0);
    assert_eq!(clamp_first(0, 40), 0);
    assert_eq!(clamp_first(38, 40), 38);
    assert_eq!(clamp_first(39, 40), 38);
    assert_eq!(clamp_first(1000, 40), 38);
    for x in [-1, 0, 1, 7] {
        assert_eq!(clamp_first(x, 1), 0);
        assert_eq!(clamp_first(x, 0), 0);
    }
}

/// `column_scroll.rs:notch_steps` flipping the sign of a wheel event scrolls the column against
/// the wheel. gpui reports wheel-down as a negative y, and wheel-down must show the next rows.
#[test]
fn a_whole_line_notch_is_one_step_in_the_wheel_direction() {
    let mut accum = 0.0;
    assert_eq!(notch_steps(-3.0, true, &mut accum, 16.0), 1);
    assert_eq!(notch_steps(1.0, true, &mut accum, 16.0), -1);
    assert_eq!(accum, 0.0);
}

/// A high-resolution wheel paying out a step per event, or never paying out, breaks the
/// one-row-per-notch feel: four quarter notches make exactly one step, on the fourth.
#[test]
fn line_fractions_and_pixels_accumulate_into_whole_steps() {
    let mut accum = 0.0;
    let steps: Vec<i32> = (0..4)
        .map(|_| notch_steps(-0.25, true, &mut accum, 16.0))
        .collect();
    assert_eq!(steps, [0, 0, 0, 1]);

    let mut accum = 0.0;
    assert_eq!(notch_steps(-10.0, false, &mut accum, 16.0), 0);
    assert_eq!(notch_steps(-10.0, false, &mut accum, 16.0), 1);
}

/// A NaN or infinite wheel distance, a NaN line height or a NaN accumulator turning into a step
/// (or poisoning the accumulator forever) makes the column jump or stop responding to the wheel.
#[test]
fn non_finite_input_never_steps_and_a_nan_accumulator_resets() {
    let mut accum = 0.0;
    assert_eq!(notch_steps(f32::NAN, true, &mut accum, 16.0), 0);
    assert_eq!(notch_steps(f32::INFINITY, false, &mut accum, 16.0), 0);
    assert_eq!(notch_steps(-10.0, false, &mut accum, f32::NAN), 0);
    assert_eq!(notch_steps(-10.0, false, &mut accum, 0.0), 0);
    let mut poisoned = f32::NAN;
    assert_eq!(notch_steps(-16.0, false, &mut poisoned, 16.0), 1);
    assert_eq!(poisoned, 0.0);
}
