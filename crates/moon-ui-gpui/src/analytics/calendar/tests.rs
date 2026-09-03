//! Calendar result-publication tests.

use std::sync::Arc;

use moon_core::db::analytics::DayCell;
use moon_core::db::{FailKind, ReadFail};

use super::{
    DAY_ROWS, ProfitLoadState, apply_calendar_results, day_window, fmt_amount, fmt_duration_short,
    fmt_volume, hour_start, next_day, previous_day, resolve_calendar_date,
};
use crate::load_state::{LoadState, Note};

/// `calendar/mod.rs:apply_calendar_results` must apply both `ReadFail` values to their
/// `LoadState`; replacing either failure with `None` renders endless Loading or a false
/// "no previous period" KPI instead of the database error.
#[test]
fn current_and_previous_failures_remain_visible() {
    let failure = || ReadFail::Failed {
        kind: FailKind::Corrupt,
        msg: Arc::from("broken calendar"),
    };
    let mut days = ProfitLoadState::<Vec<DayCell>>::default();
    let mut previous = LoadState::<Option<moon_core::db::analytics::CellTotals>>::default();

    let failed = apply_calendar_results(&mut days, &mut previous, Err(failure()), true, false);
    assert!(failed);

    assert!(matches!(
        days.view(|_| false),
        Err(Note::Failed {
            kind: FailKind::Corrupt,
            ..
        })
    ));
    assert!(matches!(
        previous.view(|_| false),
        Err(Note::Failed {
            kind: FailKind::Corrupt,
            ..
        })
    ));
}

/// `analytics/calendar/mod.rs:apply_calendar_results` must leave both settled Calendar snapshots
/// intact for a report-driven failure. Returning `false` or applying that failure clears the cells,
/// so `cal_dirty` can stop the catch-up and the visible calendar stays stale or flashes blank.
#[test]
fn report_catch_up_failure_preserves_current_and_previous_calendar_snapshots() {
    let mut days = ProfitLoadState::Ready {
        unit: None,
        data: Arc::new(vec![DayCell {
            start: 123,
            ..Default::default()
        }]),
    };
    let mut previous = LoadState::Ready(Arc::new(Some(Default::default())));
    let original_days = days.data().expect("settled Calendar cells").clone();
    let original_previous = previous
        .data()
        .expect("settled previous-period total")
        .clone();
    let failure = ReadFail::Failed {
        kind: FailKind::Busy,
        msg: Arc::from("busy calendar"),
    };

    assert!(
        apply_calendar_results(&mut days, &mut previous, Err(failure), true, true),
        "a failed period read must keep Calendar dirty for the bounded retry"
    );
    assert!(
        Arc::ptr_eq(
            &original_days,
            days.data().expect("preserved Calendar cells")
        ),
        "the catch-up failure must retain the exact current-cell snapshot"
    );
    assert!(
        Arc::ptr_eq(
            &original_previous,
            previous.data().expect("preserved previous-period total")
        ),
        "the catch-up failure must retain the exact comparison snapshot"
    );
}

/// `analytics/calendar/mod.rs:apply_calendar_results` must preserve both settled Calendar
/// snapshots for a transient Split result and keep Calendar dirty. Publishing Split unconditionally
/// flashes incomplete cells, while returning false stops the catch-up that would replace them.
#[test]
fn transient_split_preserves_calendar_snapshots_and_requests_a_catch_up() {
    let mut days = ProfitLoadState::Ready {
        unit: None,
        data: Arc::new(vec![DayCell {
            start: 456,
            ..Default::default()
        }]),
    };
    let mut previous = LoadState::Ready(Arc::new(Some(Default::default())));
    let original_days = days.data().expect("settled Calendar cells").clone();
    let original_previous = previous
        .data()
        .expect("settled previous-period total")
        .clone();

    assert!(
        apply_calendar_results(
            &mut days,
            &mut previous,
            Ok(moon_core::db::ProfitScope::Split(Default::default())),
            true,
            true,
        ),
        "a transient Split result must keep Calendar dirty for its scheduled correction"
    );
    assert!(
        Arc::ptr_eq(
            &original_days,
            days.data().expect("preserved Calendar cells")
        ),
        "a transient Split must retain the exact current-cell snapshot"
    );
    assert!(
        Arc::ptr_eq(
            &original_previous,
            previous.data().expect("preserved previous-period total")
        ),
        "a transient Split must retain the exact comparison snapshot"
    );
}

/// Replacing `calendar::hour_start`'s gap rejection with the shared picker clamp would map both
/// Warsaw 02:00 and 03:00 to the same bucket and duplicate that hour's profit in the Day grid.
#[test]
fn spring_forward_gap_has_no_duplicate_calendar_hour() {
    let zone = chrono_tz::Europe::Warsaw;
    let day = moon_core::util::display_time::day_start(
        chrono::NaiveDate::from_ymd_opt(2026, 3, 29).expect("valid date"),
        zone,
    )
    .expect("day starts");

    assert_eq!(hour_start(day, 2, zone), None);
    assert_eq!(hour_start(day, 3, zone), Some(1_774_746_000));
}

/// Replacing `resolve_calendar_date` with the shared forward-clamping `day_start` makes backward
/// navigation from Apia December 31 resolve the skipped December 30 back to December 31 and stick.
#[test]
fn backward_navigation_steps_over_a_fully_skipped_date() {
    let skipped = chrono::NaiveDate::from_ymd_opt(2011, 12, 30).expect("valid date");
    assert_eq!(
        resolve_calendar_date(skipped, chrono_tz::Pacific::Apia, -1),
        Some(1_325_152_800)
    );
    assert_eq!(
        moon_core::util::display_time::date(1_325_152_800, chrono_tz::Pacific::Apia),
        chrono::NaiveDate::from_ymd_opt(2011, 12, 29)
    );
}

/// Replacing `calendar::previous_day` with forward-clamping `day_start(shift_date(...))` makes
/// December 31 compare against itself after Apia's skipped December 30.
#[test]
fn day_comparison_uses_the_previous_existing_date() {
    let selected = moon_core::util::display_time::exact_day_start(
        chrono::NaiveDate::from_ymd_opt(2011, 12, 31).expect("valid date"),
        chrono_tz::Pacific::Apia,
    )
    .expect("December 31 exists");

    assert_eq!(
        previous_day(selected, chrono_tz::Pacific::Apia),
        1_325_152_800
    );
}

/// Replacing `day_window`'s existing-day step with a fixed calendar-date subtraction leaves only
/// 29 rendered rows when the 30-date span crosses Apia's skipped December 30.
#[test]
fn day_window_keeps_thirty_existing_rows_across_a_skipped_date() {
    let selected = moon_core::util::display_time::exact_day_start(
        chrono::NaiveDate::from_ymd_opt(2011, 12, 31).expect("valid date"),
        chrono_tz::Pacific::Apia,
    )
    .expect("December 31 exists");
    let (top, bottom) = day_window(selected, chrono_tz::Pacific::Apia);
    let mut rows = 1;
    let mut day = top;
    while day < bottom {
        day = next_day(day, chrono_tz::Pacific::Apia);
        rows += 1;
    }

    assert_eq!(rows, DAY_ROWS);
    assert_eq!(day, bottom);
}

// ── Card formatting ─────────────────────────────────────────────────────────

/// The card has one line for a holding time, so the formatter must fall back to coarser units
/// instead of growing. A third unit, or a zero second unit, overflows the cell it renders into.
#[test]
fn duration_shows_at_most_two_units() {
    assert_eq!(fmt_duration_short(45.0), "45s");
    assert_eq!(fmt_duration_short(89.0), "1m 29s");
    assert_eq!(fmt_duration_short(120.0), "2m");
    assert_eq!(fmt_duration_short(8_100.0), "2h 15m");
    assert_eq!(fmt_duration_short(7_200.0), "2h");
    assert_eq!(fmt_duration_short(273_600.0), "3d 4h");
    // Rounding happens before the split, so 59.6 s is a minute rather than "59s".
    assert_eq!(fmt_duration_short(59.6), "1m");
    assert_eq!(fmt_duration_short(0.0), "0s");
}

/// A duration that cannot exist must not render as a number.
#[test]
fn duration_rejects_impossible_input() {
    assert_eq!(fmt_duration_short(-1.0), "—");
    assert_eq!(fmt_duration_short(f64::NAN), "—");
    assert_eq!(fmt_duration_short(f64::INFINITY), "—");
}

/// Turnover lands on round hundreds constantly, and the SI formatter used to eat their zeros.
#[test]
fn volume_keeps_the_magnitude_it_was_given() {
    assert_eq!(fmt_volume(100_000.0), "100K");
    assert_eq!(fmt_volume(1_500_000.0), "1.5M");
    assert_eq!(fmt_volume(73_000.0), "73K");
    assert_eq!(fmt_volume(f64::NAN), "—");
}

/// An incomplete cost must be visibly approximate: the same string for a partial and a complete
/// total is what turns an undercount into a number the user trusts exactly.
#[test]
fn cost_marks_itself_approximate_when_trades_are_unpriced() {
    assert_eq!(fmt_amount(835.44, true), "835.44");
    assert_eq!(fmt_amount(835.44, false), "~835.44");
}

/// The commission stands beside the profit in the same KPI row, so it must be readable as the same
/// kind of number. The SI form would render a month's commission as "1.36K" next to "+560.28".
#[test]
fn cost_is_formatted_like_profit_not_compacted() {
    assert_eq!(fmt_amount(1355.41, true), "1355.41");
    assert_eq!(fmt_amount(2015.8, true), "2015.8");
    assert_eq!(fmt_amount(12.5, true), "12.5");
    assert_eq!(fmt_amount(f64::NAN, true), "—");
}
