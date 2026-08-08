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
