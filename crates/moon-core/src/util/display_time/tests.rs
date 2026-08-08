//! Regression tests for selected-zone civil boundaries and historical offsets.

use chrono::{NaiveDate, TimeZone as _, Utc};
use chrono_tz::{America::Sao_Paulo, Europe::Warsaw, Pacific::Apia};

use super::*;

/// Build a valid whole-minute picker value.
///
/// Args:
///     year: Civil year.
///     month: Civil month.
///     day: Civil day.
///     hour: Civil hour.
///     minute: Civil minute.
///
/// Returns:
///     A valid naive date and time.
fn local(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|date| date.and_hms_opt(hour, minute, 0))
        .expect("valid test civil time")
}

/// Regression target: `display_time::unix_from_local` choosing the same fall-back occurrence for
/// both bounds. That edit makes a Report range ending at Warsaw 02:30 omit the repeated hour.
#[test]
fn ambiguous_picker_bounds_cover_both_fall_back_occurrences() {
    let value = local(2026, 10, 25, 2, 30);
    let lower = unix_from_local(value, Warsaw, LocalBoundary::Lower).expect("lower occurrence");
    let upper = unix_from_local(value, Warsaw, LocalBoundary::Upper).expect("upper occurrence");

    assert_eq!(upper - lower, 3_600);
    assert_eq!(at(lower, Warsaw).map(|dt| dt.naive_local()), Some(value));
    assert_eq!(at(upper, Warsaw).map(|dt| dt.naive_local()), Some(value));
}

/// Regression target: `display_time::unix_from_local` reinterpreting a nonexistent picker minute
/// as UTC. That edit makes a Warsaw 02:30 spring-gap filter point at 04:30 local instead of 03:00.
#[test]
fn nonexistent_picker_minute_clamps_to_the_first_real_minute() {
    let resolved = unix_from_local(local(2026, 3, 29, 2, 30), Warsaw, LocalBoundary::Lower)
        .expect("post-gap minute");

    assert_eq!(
        at(resolved, Warsaw).map(|dt| dt.naive_local()),
        Some(local(2026, 3, 29, 3, 0))
    );
}

/// Regression target: `display_time::next_bucket` adding 86,400 UTC seconds. That edit makes the
/// Calendar skip Warsaw's 23-hour DST day or place the next cell at 01:00 local.
#[test]
fn civil_day_steps_across_spring_dst_without_fixed_seconds() {
    let first_date = NaiveDate::from_ymd_opt(2026, 3, 29).expect("valid date");
    let first = day_start(first_date, Warsaw).expect("first day start");
    let next = next_bucket(first, 86_400, Warsaw).expect("next civil day");

    assert_eq!(next - first, 82_800);
    assert_eq!(date(next, Warsaw), Some(shift_date(first_date, 1)));
}

/// Regression target: `display_time::bucket_start` returning a fixed UTC-day floor. That edit puts
/// a trade closed at Warsaw 00:30 into the previous Calendar date.
#[test]
fn daily_bucket_starts_at_selected_zone_midnight() {
    let instant = Utc
        .with_ymd_and_hms(2026, 3, 28, 23, 30, 0)
        .single()
        .expect("valid UTC instant")
        .timestamp();
    let bucket = bucket_start(instant, 86_400, Warsaw).expect("local day bucket");

    assert_eq!(
        at(bucket, Warsaw).map(|dt| dt.naive_local()),
        Some(local(2026, 3, 29, 0, 0))
    );
}

/// Regression target: `display_time::day_start` assuming midnight always exists. That edit cannot
/// represent Apia's dateline skip and makes a selected calendar date panic or loop forever.
#[test]
fn skipped_civil_date_resolves_to_the_next_real_date() {
    let skipped = NaiveDate::from_ymd_opt(2011, 12, 30).expect("valid skipped date");
    let resolved = day_start(skipped, Apia).expect("first instant after skipped date");

    assert_eq!(date(resolved, Apia), Some(shift_date(skipped, 1)));
}

/// Replacing `shift_day_start` with `day_start(shift_date(...))` makes a backward step from Apia
/// December 31 clamp through the skipped December 30 back onto December 31 instead of December 29.
#[test]
fn existing_day_steps_skip_a_missing_calendar_date_in_both_directions() {
    let december_29 = exact_day_start(
        NaiveDate::from_ymd_opt(2011, 12, 29).expect("valid date"),
        Apia,
    )
    .expect("December 29 exists");
    let december_31 = exact_day_start(
        NaiveDate::from_ymd_opt(2011, 12, 31).expect("valid date"),
        Apia,
    )
    .expect("December 31 exists");

    assert_eq!(shift_day_start(december_31, -1, Apia), Some(december_29));
    assert_eq!(shift_day_start(december_29, 1, Apia), Some(december_31));
}

/// Advancing from the resolved 01:00 value instead of the conceptual midnight boundary would
/// keep every day after Sao Paulo's 2018 midnight gap anchored at 01:00 and lose normal buckets.
#[test]
fn daily_step_realigns_after_a_midnight_gap() {
    let gap_date = NaiveDate::from_ymd_opt(2018, 11, 4).expect("valid date");
    let start = day_start(gap_date, Sao_Paulo).expect("gap day resolves");
    assert_eq!(
        at(start, Sao_Paulo).map(|value| value.naive_local()),
        Some(local(2018, 11, 4, 1, 0))
    );

    let next = next_bucket(start, 86_400, Sao_Paulo).expect("next day resolves");
    assert_eq!(
        at(next, Sao_Paulo).map(|value| value.naive_local()),
        Some(local(2018, 11, 5, 0, 0))
    );
}

/// Replacing `format_utc_millis_clock` with the stored suffix makes visible Log rows remain UTC
/// after the application-wide display zone changes.
#[test]
fn stored_utc_log_clocks_follow_the_selected_zone() {
    assert_eq!(
        format_utc_millis_clock("2026-07-25 08:26:50.123", Warsaw),
        "10:26:50.123"
    );
    assert_eq!(format_utc_millis_clock("malformed 12:34", Warsaw), "12:34");
}
