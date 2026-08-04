//! Regression coverage for the shared from/to bound arithmetic.

use chrono::NaiveDate;

use crate::controls::date_range::{
    Bound, MINUTE, dt_of_secs, exclusive_end, field_of_exclusive, field_of_inclusive,
    inclusive_end, secs_of_dt,
};

/// Build a UTC timestamp the way a field holds one.
fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> chrono::NaiveDateTime {
    NaiveDate::from_ymd_opt(year, month, day)
        .expect("valid test date")
        .and_hms_opt(hour, minute, 0)
        .expect("valid test time")
}

/// The rule the whole change rests on: an untouched lower field means the START of a day and an
/// untouched upper one the END of it.
///
/// Catches giving both edges the picker's own 00:00 default, which would turn "04.08 – 04.08" into
/// a single midnight minute and report an empty day.
#[test]
fn the_two_edges_default_to_opposite_ends_of_the_day() {
    assert_eq!(Bound::From.default_time(), at(2026, 8, 4, 0, 0).time());
    assert_eq!(Bound::To.default_time(), at(2026, 8, 4, 23, 59).time());
}

/// Catches an upper bound that stops at the first second of the picked minute, which would drop
/// the 59 seconds the user believes they selected and make an equal from/to pair select nothing.
#[test]
fn an_upper_bound_covers_its_whole_minute() {
    let picked = secs_of_dt(at(2026, 8, 4, 9, 30));

    assert_eq!(inclusive_end(picked), picked + 59);
    assert_eq!(exclusive_end(picked), picked + MINUTE);
    assert_eq!(exclusive_end(picked) - inclusive_end(picked), 1);
}

/// The whole-day case, expressed in the two bound flavours: a day picked on both fields covers
/// 86_400 seconds either way.
#[test]
fn a_single_day_spans_a_whole_day_in_both_flavours() {
    let from = secs_of_dt(at(2026, 8, 4, 0, 0));
    let to = secs_of_dt(at(2026, 8, 4, 23, 59));

    assert_eq!(inclusive_end(to) - from + 1, 86_400);
    assert_eq!(exclusive_end(to) - from, 86_400);
}

/// Catches showing the exclusive edge itself in the upper field, which would render 05.08 00:00
/// for a range entered as 04.08 23:59 and creep forward a minute on every round trip.
#[test]
fn a_stored_bound_reads_back_as_the_minute_the_user_picked() {
    let picked = at(2026, 8, 4, 17, 45);
    let secs = secs_of_dt(picked);

    assert_eq!(field_of_inclusive(inclusive_end(secs)), Some(picked));
    assert_eq!(field_of_exclusive(exclusive_end(secs)), Some(picked));
}

/// Catches breaking bounds written by an older, date-only build: a whole day was stored as
/// `midnight + 86_399` (Report) or as the next midnight (Analytics), and both must still read back
/// as that day's last minute.
#[test]
fn legacy_day_aligned_bounds_read_back_as_that_days_last_minute() {
    let midnight = secs_of_dt(at(2026, 8, 4, 0, 0));

    assert_eq!(
        field_of_inclusive(midnight + 86_399),
        Some(at(2026, 8, 4, 23, 59))
    );
    assert_eq!(
        field_of_exclusive(midnight + 86_400),
        Some(at(2026, 8, 4, 23, 59))
    );
}

/// Catches a seconds-carrying bound landing in a field that can only show minutes: it must floor
/// into the range, never round up past it.
#[test]
fn a_bound_between_minutes_floors_into_the_range() {
    let secs = secs_of_dt(at(2026, 8, 4, 9, 30)) + 17;

    assert_eq!(field_of_inclusive(secs), Some(at(2026, 8, 4, 9, 30)));
}

/// Catches an off-by-one in the seconds↔value conversion itself, which every bound above builds on.
#[test]
fn the_value_conversion_round_trips() {
    let value = at(2026, 8, 4, 9, 30);

    assert_eq!(dt_of_secs(secs_of_dt(value)), Some(value));
}
