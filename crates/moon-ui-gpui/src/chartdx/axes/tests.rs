//! Regression coverage for selected-zone chart-axis boundaries.

use chrono::Timelike as _;

use super::aligned_ticks_ms_in_zone;

/// Replacing `axes::aligned_ticks_ms_in_zone` with one offset captured at the left edge would
/// produce 00:00, 07:00, and 13:00 labels across Warsaw's spring transition instead of keeping
/// the six-hour civil grid at 00:00, 06:00, and 12:00.
#[test]
fn chart_ticks_realign_after_a_dst_offset_change() {
    let zone = chrono_tz::Europe::Warsaw;
    let left = 1_774_738_800_000.0; // 2026-03-29 00:00 Warsaw.
    let right = 1_774_782_000_000.0; // 2026-03-29 13:00 Warsaw.

    let ticks = aligned_ticks_ms_in_zone(left, right, 6.0 * 3_600_000.0, zone);
    let hours: Vec<_> = ticks
        .iter()
        .filter_map(|tick| moon_core::util::display_time::at_millis(*tick as i64, zone))
        .map(|value| value.hour())
        .collect();

    assert_eq!(hours, vec![0, 6, 12]);
    assert_eq!(ticks[1] - ticks[0], 5.0 * 3_600_000.0);
}

/// Keeping only the earlier ambiguous boundary drops the second Warsaw 02:00 and leaves a
/// two-hour visual gap; appending pairs without the final sort also scrambles sub-hour grids.
#[test]
fn fall_back_axis_keeps_both_real_hour_boundaries_in_order() {
    let ticks = aligned_ticks_ms_in_zone(
        1_792_879_200_000.0,
        1_792_897_200_000.0,
        3_600_000.0,
        chrono_tz::Europe::Warsaw,
    );

    assert_eq!(
        ticks,
        vec![
            1_792_879_200_000.0,
            1_792_882_800_000.0,
            1_792_886_400_000.0,
            1_792_890_000_000.0,
            1_792_893_600_000.0,
            1_792_897_200_000.0,
        ]
    );
    assert!(
        ticks
            .windows(2)
            .all(|pair| pair[1] - pair[0] == 3_600_000.0)
    );
}

/// Rounding sub-minute axis boundaries to minutes would erase the super-zoom time scale.
#[test]
fn super_zoom_axis_keeps_three_distinct_second_boundaries() {
    let left = 1_700_000_000_250.0;
    let ticks = aligned_ticks_ms_in_zone(left, left + 3000.0, 1000.0, chrono_tz::UTC);
    assert_eq!(
        ticks,
        vec![
            1_700_000_001_000.0,
            1_700_000_002_000.0,
            1_700_000_003_000.0
        ]
    );
}

/// Week steps start the week: anchored on the epoch the lattice would fall on Thursdays.
#[test]
fn week_ticks_fall_on_monday_midnights() {
    use chrono::Datelike as _;
    let zone = chrono_tz::Europe::Warsaw;
    let left = 1_767_222_000_000.0; // 2026-01-01 00:00 Warsaw, a Thursday.
    let right = left + 60.0 * 86_400_000.0;
    let ticks = aligned_ticks_ms_in_zone(left, right, 7.0 * 86_400_000.0, zone);
    assert!(ticks.len() >= 8);
    for tick in ticks {
        let local = moon_core::util::display_time::at_millis(tick as i64, zone).unwrap();
        assert_eq!(local.weekday(), chrono::Weekday::Mon);
        assert_eq!((local.hour(), local.minute()), (0, 0));
    }
}

/// Month steps fall on local first-of-month midnights, across a DST change, and three months
/// fall on quarters; a fixed 30-day lattice would drift off the first within two months.
#[test]
fn month_ticks_fall_on_local_firsts_and_quarters() {
    use chrono::Datelike as _;
    let zone = chrono_tz::Europe::Warsaw;
    let left = 1_767_222_000_000.0; // 2026-01-01 00:00 Warsaw.
    let right = left + 400.0 * 86_400_000.0;
    let month = moon_chart::axes::MONTH_SECS * 1000.0;
    let dates = |step: f64| -> Vec<(i32, u32, u32, u32)> {
        aligned_ticks_ms_in_zone(left, right, step, zone)
            .into_iter()
            .map(|tick| {
                let local = moon_core::util::display_time::at_millis(tick as i64, zone).unwrap();
                (local.year(), local.month(), local.day(), local.hour())
            })
            .collect()
    };
    let monthly = dates(month);
    assert_eq!(monthly.len(), 14);
    assert!(
        monthly
            .iter()
            .all(|&(_, _, day, hour)| day == 1 && hour == 0)
    );
    assert_eq!(monthly[3], (2026, 4, 1, 0));
    let quarterly: Vec<u32> = dates(3.0 * month).iter().map(|d| d.1).collect();
    assert_eq!(quarterly, vec![1, 4, 7, 10, 1]);
}

/// Havana springs forward at midnight (2026-03-08 00:00 → 01:00), so that day has no 00:00.
/// A day step must still caption it, at the first real instant, rather than drop the date.
#[test]
fn a_midnight_skipped_by_dst_still_gets_its_date_tick() {
    use chrono::{Datelike as _, TimeZone as _};
    let zone = chrono_tz::America::Havana;
    let left = zone
        .with_ymd_and_hms(2026, 3, 6, 0, 0, 0)
        .unwrap()
        .timestamp_millis() as f64;
    let right = left + 4.5 * 86_400_000.0;
    let ticks = aligned_ticks_ms_in_zone(left, right, 86_400_000.0, zone);
    let days: Vec<(u32, u32)> = ticks
        .iter()
        .map(|tick| {
            let local = moon_core::util::display_time::at_millis(*tick as i64, zone).unwrap();
            (local.day(), local.hour())
        })
        .collect();
    assert_eq!(days, vec![(6, 0), (7, 0), (8, 1), (9, 0), (10, 0)]);
}
