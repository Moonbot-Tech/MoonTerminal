//! Regression coverage for the minute-precise bounds of a custom Analytics period.

use chrono::{NaiveDate, TimeZone as _, Utc};

use super::{Period, Tab, custom_bounds as zoned_custom_bounds, exact_secs_of_day, seed_period};
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
    secs_of_dt(at(year, month, day, 0, 0), chrono_tz::UTC, Bound::From)
        .expect("UTC midnight resolves")
}

/// Apply the production custom-bound policy in UTC for legacy arithmetic assertions.
///
/// Args:
///     from: Optional lower picker value.
///     to: Optional upper picker value.
///     tomorrow: UTC midnight after the test's current day.
///
/// Returns:
///     Production `[from, to)` bounds resolved in UTC.
fn custom_bounds(
    from: Option<chrono::NaiveDateTime>,
    to: Option<chrono::NaiveDateTime>,
    tomorrow: i64,
) -> (i64, i64) {
    zoned_custom_bounds(from, to, tomorrow, chrono_tz::UTC)
}

/// Replacing `seed_period`'s `Tab::Strategies => strat` arm with `summary` restores the wrong
/// picker range after reopening Strategy Tuning, hiding its active custom filter from the user.
#[test]
fn seed_period_uses_the_reopened_tabs_persisted_axis() {
    let summary = Period::Custom(1_701_000_000, 1_701_086_400);
    let strategy = Period::Custom(1_702_000_000, 1_702_172_800);

    assert!(
        seed_period(Tab::Strategies, Some(summary), Some(strategy)) == Some(strategy),
        "Strategy Tuning must reopen with its own custom period"
    );
    assert!(seed_period(Tab::Summary, Some(summary), Some(strategy)) == Some(summary));
    assert!(seed_period(Tab::Calendar, Some(summary), Some(strategy)) == Some(summary));
    assert!(seed_period(Tab::Strategies, Some(summary), None).is_none());
    assert!(seed_period(Tab::Summary, None, Some(strategy)).is_none());
}

/// Removing `exact_secs_of_day`'s civil-date identity check makes both cells resolve to December
/// 31 and duplicates that day's profit in Calendar Month for the Apia dateline transition.
#[test]
fn a_fully_skipped_civil_date_has_no_month_cell_bucket() {
    let skipped = NaiveDate::from_ymd_opt(2011, 12, 30).expect("valid date");
    let following = NaiveDate::from_ymd_opt(2011, 12, 31).expect("valid date");

    assert_eq!(exact_secs_of_day(skipped, chrono_tz::Pacific::Apia), None);
    assert_eq!(
        exact_secs_of_day(following, chrono_tz::Pacific::Apia),
        Some(1_325_239_200)
    );
}

/// Replacing `Period::range_at`'s existing-day step with `day_start(shift_date(...))` makes Apia
/// Yesterday empty on December 31 because the skipped December 30 clamps forward onto today.
#[test]
fn yesterday_uses_the_previous_existing_civil_date() {
    let now = Utc
        .with_ymd_and_hms(2011, 12, 30, 12, 0, 0)
        .single()
        .expect("valid UTC instant")
        .timestamp();

    assert_eq!(
        Period::Yesterday.range_at(now, chrono_tz::Pacific::Apia),
        (1_325_152_800, 1_325_239_200)
    );
}

/// Dropping `Period::range_at`'s CurYear `.with_month(1)` step leaves `today.with_day(1)` like
/// CurMonth, so the This year chip filters the current month, not the civil year.
///
/// Independent oracle: pin `now` at 2024-06-15 12:00 UTC. January 1 and tomorrow are literal
/// `chrono::Utc` midnights, not values `range_at` computed. Exclusive end is tomorrow of the
/// pinned civil day, not Report's open `None` upper bound. Apia repeats the same pin in a
/// UTC+13 zone so January 1 is not UTC midnight.
#[test]
fn cur_year_range_starts_january_1_and_ends_tomorrow() {
    let now = Utc
        .with_ymd_and_hms(2024, 6, 15, 12, 0, 0)
        .single()
        .expect("valid UTC instant")
        .timestamp();
    let year_start = Utc
        .with_ymd_and_hms(2024, 1, 1, 0, 0, 0)
        .single()
        .expect("valid UTC instant")
        .timestamp();
    let tomorrow = Utc
        .with_ymd_and_hms(2024, 6, 16, 0, 0, 0)
        .single()
        .expect("valid UTC instant")
        .timestamp();

    assert_eq!(
        Period::CurYear.range_at(now, chrono_tz::UTC),
        (year_start, tomorrow),
        "CurYear is [Jan 1 00:00, tomorrow) of the pinned day, not the current month"
    );

    // 2024-06-15 12:00 UTC is 2024-06-16 01:00 in Pacific/Apia (UTC+13).
    let apia_year_start = chrono_tz::Pacific::Apia
        .with_ymd_and_hms(2024, 1, 1, 0, 0, 0)
        .single()
        .expect("Apia Jan 1 midnight exists")
        .timestamp();
    let apia_tomorrow = chrono_tz::Pacific::Apia
        .with_ymd_and_hms(2024, 6, 17, 0, 0, 0)
        .single()
        .expect("Apia Jun 17 midnight exists")
        .timestamp();

    assert_eq!(
        Period::CurYear.range_at(now, chrono_tz::Pacific::Apia),
        (apia_year_start, apia_tomorrow),
        "CurYear uses the selected zone's January 1 and civil tomorrow"
    );
}

/// `analytics/period.rs:Period::range_at` must resolve Last month as the entire previous civil
/// month; replacing `prev_month_start` with the current year in January makes all Analytics tabs
/// silently show the wrong calendar month after New Year.
///
/// Independent oracle: each edge is a separately pinned UTC civil midnight, including January's
/// prior-year December, a 31-day predecessor, and both February lengths. These dates do not come
/// from `range_at` or from the shared carry helper.
#[test]
fn last_month_range_uses_previous_calendar_month_boundaries() {
    let midnight = |year, month, day| {
        Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
            .single()
            .expect("valid UTC midnight")
            .timestamp()
    };

    for (now, expected, why) in [
        (
            midnight(2024, 6, 15),
            (midnight(2024, 5, 1), midnight(2024, 6, 1)),
            "mid-year month",
        ),
        (
            midnight(2024, 1, 15),
            (midnight(2023, 12, 1), midnight(2024, 1, 1)),
            "January rolls into the prior year",
        ),
        (
            midnight(2024, 9, 15),
            (midnight(2024, 8, 1), midnight(2024, 9, 1)),
            "31-day August precedes a 30-day current month",
        ),
        (
            midnight(2024, 3, 15),
            (midnight(2024, 2, 1), midnight(2024, 3, 1)),
            "leap-year February ends on the following month start",
        ),
        (
            midnight(2023, 3, 15),
            (midnight(2023, 2, 1), midnight(2023, 3, 1)),
            "ordinary February has the same month-start boundary shape",
        ),
    ] {
        assert_eq!(
            Period::LastMonth.range_at(now, chrono_tz::UTC),
            expected,
            "{why}"
        );
    }
}

/// `analytics/period.rs:Period::from_id` must restore the Last month id already persisted in
/// Analytics layout; renaming it drops a user's chosen preset on the next window reopen.
#[test]
fn last_month_persisted_id_round_trips_to_its_preset() {
    assert!(Period::from_id("p-last-month") == Some(Period::LastMonth));
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
    assert_eq!(
        lower,
        secs_of_dt(pick, chrono_tz::UTC, Bound::From).expect("UTC value resolves")
    );
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

    assert_eq!(field_of_exclusive(upper, chrono_tz::UTC), Some(to));
}

/// Catches labelling the period with the exclusive edge, which would advertise a minute the range
/// does not contain.
#[test]
fn the_period_label_names_the_last_minute_inside_the_range() {
    let label = Period::Custom(midnight(2026, 8, 4), midnight(2026, 8, 5)).title(chrono_tz::UTC);

    assert_eq!(label, "04.08.26 00:00 – 04.08.26 23:59");
}
