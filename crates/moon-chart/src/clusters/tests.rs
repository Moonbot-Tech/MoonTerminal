//! Unit tests for the cluster grid, the sums and the disc sizes.

use super::*;

fn grid(time_step_ms: i64, tick: f64, price_step: u32) -> ClusterGrid {
    ClusterGrid {
        time_step_ms,
        tick,
        price_step,
    }
}

#[test]
fn the_time_step_is_the_finest_that_keeps_a_cell_wide_enough() {
    // 0.001 px per ms: a second is 1 px, so 14 px needs the 15 s interval.
    assert_eq!(time_step_ms(0.001, 14.0), Some(15_000));
    // 0.02 px per ms: a second is already 20 px.
    assert_eq!(time_step_ms(0.02, 14.0), Some(1_000));
    assert_eq!(time_step_ms(0.0, 14.0), None);
    // So far out that a day is under a pixel.
    assert_eq!(time_step_ms(1e-9, 14.0), None);
}

#[test]
fn trades_sum_into_their_cell_by_side() {
    let mut out = Vec::new();
    aggregate(
        [
            (1_000.0, 100.0, 1.0, true),
            (1_500.0, 99.99, 2.0, false),
            (2_100.0, 100.04, 1.0, true),
        ],
        grid(1_000, 0.01, 2),
        &mut out,
    );
    // Two ticks a row: 99.99 and 100.00 share the row centred on 100.00, the nearest; 100.04 is
    // two rows up, so nothing merges.
    assert_eq!(out.len(), 2);
    assert_eq!((out[0].time_idx, out[0].price_idx), (1, 5_000));
    assert_eq!(out[0].buy, 100.0);
    assert!((out[0].sell - 199.98).abs() < 1e-3);
    assert_eq!(out[0].center_ms, 1_500.0);
    assert_eq!((out[1].time_idx, out[1].price_idx), (2, 5_002));
    assert!((out[1].buy - 100.04).abs() < 1e-3);
}

#[test]
fn intervals_are_aligned_to_absolute_time() {
    let mut out = Vec::new();
    aggregate(
        [(59_999.0, 10.0, 1.0, true), (60_000.0, 12.0, 1.0, true)],
        grid(60_000, 1.0, 1),
        &mut out,
    );
    let idx: Vec<(i64, i64)> = out.iter().map(|c| (c.price_idx, c.time_idx)).collect();
    assert_eq!(idx, vec![(10, 0), (12, 1)]);
}

#[test]
fn consecutive_intervals_on_a_row_merge_up_to_the_span() {
    let mut out = Vec::new();
    aggregate(
        [
            // Row 10: intervals 0, 1, 2 merge; 3 starts the next cluster (the span is full);
            // 5 stands alone after the empty 4.
            (100.0, 10.0, 1.0, true),
            (1_100.0, 10.0, 1.0, false),
            (2_100.0, 10.0, 4.0, true),
            (3_100.0, 10.0, 1.0, true),
            (5_100.0, 10.0, 1.0, true),
            // Row 11, interval 1: a neighbouring row never joins row 10's cluster.
            (1_200.0, 11.0, 1.0, true),
        ],
        grid(1_000, 1.0, 1),
        &mut out,
    );
    let shape: Vec<(i64, i64)> = out.iter().map(|c| (c.price_idx, c.time_idx)).collect();
    assert_eq!(shape, vec![(10, 0), (10, 3), (10, 5), (11, 1)]);
    let first = out[0];
    assert_eq!((first.buy, first.sell), (50.0, 10.0));
    // Volumes 10, 10, 40 at offsets 0, 1, 2: the centre sits 1.5 intervals past the first one's.
    assert_eq!(first.center_ms, 2_000.0);
    assert_eq!(out[1].center_ms, 3_500.0);
    assert_eq!(out[2].center_ms, 5_500.0);
}

#[test]
fn unusable_trades_and_grids_add_nothing() {
    let mut out = Vec::new();
    aggregate(
        [
            (f64::NAN, 10.0, 1.0, true),
            (1.0, 10.0, 0.0, true),
            (1.0, f32::NAN, 1.0, true),
        ],
        grid(1_000, 1.0, 1),
        &mut out,
    );
    assert!(out.is_empty());
    aggregate([(1.0, 10.0, 1.0, true)], grid(0, 1.0, 1), &mut out);
    assert!(out.is_empty());
}

#[test]
fn a_disc_s_area_follows_its_volume_against_the_biggest() {
    let max_r = max_radius(30.0, 20.0);
    assert_eq!(max_r, 9.0);
    assert_eq!(disc_radius(400.0, 400.0, max_r), 9.0);
    // A quarter of the volume: a quarter of the area, half the radius.
    assert_eq!(disc_radius(100.0, 400.0, max_r), 4.5);
    // A speck still shows.
    assert_eq!(disc_radius(1e-6, 400.0, max_r), MIN_DISC_PX);
}

#[test]
fn a_volume_prints_with_at_most_three_digits() {
    assert_eq!(short_volume(10_800.0), "11K");
    assert_eq!(short_volume(2_340_000.0), "2.3M");
    assert_eq!(short_volume(2_000_000.0), "2M");
    assert_eq!(short_volume(906.0), "906");
    assert_eq!(short_volume(15_000_000.0), "15M");
    // Rounding into the next unit prints that unit, not "1000K".
    assert_eq!(short_volume(999_700.0), "1M");
    assert_eq!(short_volume(0.4), "0.4");
    // Below one: no "0.0" that reads as an empty cell, no "1.0" beside a plain "1".
    assert_eq!(short_volume(0.04), "<0.1");
    assert_eq!(short_volume(0.97), "1");
    assert_eq!(short_volume(999.7), "1K");
    assert_eq!(short_volume(f32::NAN), "");
}
