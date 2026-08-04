//! Regression coverage for the minute-precise bounds of a custom Analytics period.

use chrono::NaiveDate;

use super::{Period, custom_bounds};
use crate::controls::date_range::{Bound, MINUTE, field_of_exclusive, secs_of_dt};

/// Build a UTC timestamp out of a day and a clock time, the way the pickers hold their value.
fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> chrono::NaiveDateTime {
    NaiveDate::from_ymd_opt(year, month, day)
        .expect("valid test date")
        .and_hms_opt(hour, minute, 0)
        .expect("valid test time")
}

/// Midnight of a day in UTC unix seconds.
fn midnight(year: i32, month: u32, day: u32) -> i64 {
    secs_of_dt(at(year, month, day, 0, 0))
}

/// The question this change had to answer: one day chosen on BOTH fields, with the two default
/// clock times, must still mean that whole day.
///
/// Catches defaulting the upper field to 00:00 like the lower one, which would collapse
/// "04.08 – 04.08" to the single midnight minute and report an almost empty day.
#[test]
fn the_same_day_on_both_fields_spans_that_whole_day() {
    let day = NaiveDate::from_ymd_opt(2026, 8, 4).expect("valid test date");
    let from = day.and_time(Bound::From.default_time());
    let to = day.and_time(Bound::To.default_time());

    let (lower, upper) = custom_bounds(Some(from), Some(to), midnight(2026, 8, 5));

    assert_eq!(lower, midnight(2026, 8, 4));
    assert_eq!(
        upper,
        midnight(2026, 8, 5),
        "23:59 is inclusive, so the range must end at the following midnight"
    );
    assert_eq!(upper - lower, 86_400);
}

/// Catches treating the upper bound as exclusive-at-the-picked-minute, which would make an equal
/// from/to pair select nothing at all.
#[test]
fn an_equal_from_and_to_still_covers_the_picked_minute() {
    let pick = at(2026, 8, 4, 9, 30);

    let (lower, upper) = custom_bounds(Some(pick), Some(pick), midnight(2026, 8, 5));

    assert_eq!(upper - lower, MINUTE);
    assert_eq!(lower, secs_of_dt(pick));
}

/// Catches dropping the clock time when composing the bounds — the whole point of the control.
#[test]
fn the_bounds_keep_the_picked_clock_time() {
    let (lower, upper) = custom_bounds(
        Some(at(2026, 8, 4, 9, 30)),
        Some(at(2026, 8, 4, 17, 45)),
        midnight(2026, 8, 5),
    );

    assert_eq!(lower, midnight(2026, 8, 4) + 9 * 3600 + 30 * 60);
    assert_eq!(upper, midnight(2026, 8, 4) + 17 * 3600 + 46 * 60);
}

/// Catches turning an empty field into "now" or into midnight: an empty lower field means the
/// whole history, an empty upper one means up to tomorrow.
#[test]
fn an_empty_field_leaves_that_edge_open() {
    let tomorrow = midnight(2026, 8, 5);

    let (lower, upper) = custom_bounds(None, Some(at(2026, 8, 4, 23, 59)), tomorrow);
    assert_eq!(lower, -1);
    assert_eq!(upper, tomorrow);

    let (lower, upper) = custom_bounds(Some(at(2026, 8, 4, 0, 0)), None, tomorrow);
    assert_eq!(lower, midnight(2026, 8, 4));
    assert_eq!(upper, tomorrow);
}

/// Catches producing an inverted range when the lower bound is picked past today and the upper
/// field is left empty: the implicit "until tomorrow" edge must not precede the picked bound.
#[test]
fn a_future_lower_bound_without_an_upper_one_stays_non_empty() {
    let (lower, upper) = custom_bounds(Some(at(2026, 8, 9, 12, 0)), None, midnight(2026, 8, 5));

    assert!(
        upper > lower,
        "an open upper edge must not invert the range"
    );
}

/// Catches showing the exclusive edge itself in the "to" field, which would display 05.08 00:00
/// for a range the user entered as 04.08 23:59 and would creep a minute forward on every reopen.
#[test]
fn the_to_field_shows_the_last_minute_inside_the_range() {
    let from = at(2026, 8, 4, 9, 30);
    let to = at(2026, 8, 4, 17, 45);
    let (_, upper) = custom_bounds(Some(from), Some(to), midnight(2026, 8, 5));

    assert_eq!(field_of_exclusive(upper), Some(to));
}

/// Catches labelling the period with the exclusive edge, which would advertise a minute the range
/// does not contain.
#[test]
fn the_period_label_names_the_last_minute_inside_the_range() {
    let label = Period::Custom(midnight(2026, 8, 4), midnight(2026, 8, 5)).title();

    assert_eq!(label, "04.08.26 00:00 – 04.08.26 23:59");
}
