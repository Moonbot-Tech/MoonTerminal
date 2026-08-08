//! Calendar result-publication tests.

use std::sync::Arc;

use moon_core::db::analytics::DayCell;
use moon_core::db::{FailKind, ReadFail};

use super::{
    DAY_ROWS, ProfitLoadState, apply_calendar_results, day_window, hour_start, next_day,
    previous_day, resolve_calendar_date, rezone_day,
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
    let mut previous = LoadState::<Option<(f64, i64, i64)>>::default();

    let failed = apply_calendar_results(&mut days, &mut previous, Err(failure()), true);
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

/// Re-bucketing Warsaw midnight directly in New York turns August 8 into August 7; removing the
/// old-zone date extraction makes Calendar Day jump backward after the user changes the city.
#[test]
fn zone_change_preserves_the_selected_calendar_date() {
    let old_zone = chrono_tz::Europe::Warsaw;
    let new_zone = chrono_tz::America::New_York;
    let date = chrono::NaiveDate::from_ymd_opt(2026, 8, 8).expect("valid date");
    let old_day = moon_core::util::display_time::day_start(date, old_zone).expect("Warsaw day");

    let rezoned = rezone_day(old_day, old_zone, new_zone);

    assert_eq!(
        moon_core::util::display_time::date(rezoned, new_zone),
        Some(date)
    );
    assert_eq!(rezoned, 1_786_161_600); // 2026-08-08 00:00 EDT.
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
