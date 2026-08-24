//! What the screen divider does at each count, in each mode, and where it refuses to divide.

// NOT `use super::*`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use super::{GridInput, effective_columns, row_count, row_slots, slots_of_rows};

/// A stack 1000 px tall and 1920 px wide, whose slots want 300 px — the COMPRESS case, where the
/// mode's own slot size says when the charts have stopped fitting. Three rows fit.
fn measured(columns: u8, exact: bool, count: usize) -> GridInput {
    GridInput {
        columns,
        exact,
        count,
        axis_extent: 1000.0,
        cross_extent: 1920.0,
        min_cross: super::MIN_COLUMN_W,
        slot_extent: 300.0,
    }
}

#[test]
fn a_divider_of_one_is_the_stack_as_it_always_was() {
    for count in [0, 1, 5, 50] {
        assert_eq!(effective_columns(measured(1, false, count)), 1);
        assert_eq!(effective_columns(measured(1, true, count)), 1);
    }
}

/// Exact means exact: the number is a layout the reader chose, not a ceiling to approach.
#[test]
fn exact_holds_the_divider_at_every_count() {
    for count in [1, 2, 3, 7, 40] {
        assert_eq!(effective_columns(measured(3, true, count)), 3);
    }
}

/// Without "exact" the stack stays one column while the charts still fit down the screen, then
/// takes a column at a time — 1/1, then 1/2, then 1/3 — until the divider is reached.
#[test]
fn columns_are_taken_only_as_the_charts_stop_fitting() {
    // Three rows fit (1000 / 300), so up to three charts stay in one column.
    for count in [1, 2, 3] {
        assert_eq!(effective_columns(measured(3, false, count)), 1, "{count}");
    }
    for count in [4, 5, 6] {
        assert_eq!(effective_columns(measured(3, false, count)), 2, "{count}");
    }
    for count in [7, 8, 9] {
        assert_eq!(effective_columns(measured(3, false, count)), 3, "{count}");
    }
    // Past the divider the stack keeps three columns and the mode does what it always did —
    // compress, scroll or stretch.
    assert_eq!(effective_columns(measured(3, false, 30)), 3);
}

/// The first frame of a stack has no bounds yet. One column is what it did before the divider
/// existed, and the measured frame that follows corrects it — nothing is laid out on a guess.
#[test]
fn an_unmeasured_stack_stays_at_one_column() {
    let mut input = measured(3, false, 9);
    input.axis_extent = 0.0;
    assert_eq!(effective_columns(input), 1);

    let mut input = measured(3, false, 9);
    input.slot_extent = 0.0;
    assert_eq!(effective_columns(input), 1);

    // Exact needs no measurement at all: the divider IS the answer.
    let mut input = measured(3, true, 9);
    input.axis_extent = 0.0;
    input.slot_extent = 0.0;
    assert_eq!(effective_columns(input), 3);
}

/// A tab with the order book switched off is not paying for the control zone, so the same window
/// holds more columns. Pinned because the wide floor applied to a book-less tab is what made the
/// divider look broken: on a narrow window it never divided, however many charts arrived.
#[test]
fn a_tab_without_an_order_book_divides_a_narrower_window() {
    let mut input = measured(2, false, 10);
    input.cross_extent = 430.0; // one 240-wide column, but three 140-wide ones
    input.min_cross = super::MIN_COLUMN_W;
    assert_eq!(effective_columns(input), 1, "with the book: no room to divide");
    input.min_cross = super::MIN_COLUMN_W_NO_BOOK;
    assert_eq!(effective_columns(input), 2, "without it: the divider applies");
}

/// A narrow window cannot hold three charts side by side, so the divider working up on its own
/// stops where a column would stop being a chart.
#[test]
fn a_narrow_stack_takes_fewer_columns_than_asked() {
    let mut input = measured(3, false, 9);
    input.cross_extent = 500.0; // two columns of 240 fit; three would not
    assert_eq!(effective_columns(input), 2);

    input.cross_extent = 100.0; // not even one — one column is the floor
    assert_eq!(effective_columns(input), 1);
}

/// A horizontal stack divides its cross extent VERTICALLY, so its floor is a row height — applying
/// the column width there would refuse rows a 900-tall window can hold perfectly well.
#[test]
fn a_horizontal_stack_is_bounded_by_a_row_height() {
    let mut input = measured(3, false, 9);
    input.cross_extent = 400.0; // as a height: four 90px rows fit; as a width: only one column
    input.min_cross = super::MIN_ROW_H;
    assert_eq!(effective_columns(input), 3);
    input.min_cross = super::MIN_COLUMN_W;
    assert_eq!(effective_columns(input), 1, "the width floor is stricter");
}

/// But an exact division is never narrowed: the reader asked for a layout and can see the result.
#[test]
fn exact_is_not_narrowed_by_a_small_window() {
    let mut input = measured(3, true, 9);
    input.cross_extent = 100.0;
    assert_eq!(effective_columns(input), 3);
}

/// The divider is held to its own ceiling, however the setting was stored.
#[test]
fn the_divider_is_clamped_to_the_supported_range() {
    let mut input = measured(200, true, 9);
    input.columns = 200;
    assert_eq!(effective_columns(input), usize::from(super::MAX_COLUMNS));

    input.columns = 0;
    assert_eq!(effective_columns(input), 1);
}

/// Rows are filled left to right; the last one stays short rather than stretching to fill.
#[test]
fn rows_are_row_major_and_the_tail_stays_short() {
    assert_eq!(row_count(3, 7), 3);
    assert_eq!(row_slots(0, 3, 7), 0..3);
    assert_eq!(row_slots(1, 3, 7), 3..6);
    assert_eq!(row_slots(2, 3, 7), 6..7);
    // A row past the end is empty rather than out of range.
    assert_eq!(row_slots(9, 3, 7), 7..7);
    assert_eq!(row_count(3, 0), 0);
}

/// Visible ROWS become visible SLOTS: this is what keeps the scrolling stack from waking the wrong
/// charts' own pass and leaving the visible ones dark.
#[test]
fn visible_rows_expand_to_the_slots_they_hold() {
    assert_eq!(slots_of_rows(0..2, 3, 10), 0..6);
    assert_eq!(slots_of_rows(2..4, 3, 10), 6..10, "clamped to the count");
    assert_eq!(slots_of_rows(0..1, 1, 10), 0..1, "one column is unchanged");
    assert_eq!(slots_of_rows(5..9, 3, 10), 10..10, "past the end is empty");
}
