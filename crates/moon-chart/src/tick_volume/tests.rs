//! Regression cases for quote units, overlapping ticks, and pending native uploads.

use super::{
    TickVolumeRange, cursor_column, cursor_time, nearby_ticks, pending_ring, quote_notional,
    tick_ranges_visible,
};

/// The whole plot answers, not only the band along its floor; DPI scales the pick column once.
///
/// Breakage this pins: restoring the former band-only Y gate — `base - band ..= base` around the
/// plot's floor — makes every candle-plot cursor here read `None` again.
#[test]
fn cursor_column_covers_the_whole_plot() {
    let bounds = [100.0, 50.0, 600.0, 800.0]; // plot 100..700 x 50..850 device pixels
    // The former band ceiling sat at 777: a cursor over the candles well above it now answers.
    for y in [50.0, 300.0, 776.0, 800.0, 850.0] {
        assert_eq!(
            cursor_column(bounds, 1_000.0, 2.0, [300.0, y], 2.0),
            Some((1097.0, 1103.0)),
            "cursor at y={y} is inside the plot"
        );
    }
    // Outside the plot rectangle on any edge, there is nothing to describe.
    for cursor in [[300.0, 49.0], [300.0, 851.0], [99.0, 800.0], [701.0, 800.0]] {
        assert_eq!(cursor_column(bounds, 1_000.0, 2.0, cursor, 2.0), None);
    }
    assert_eq!(
        cursor_column(bounds, 1_000.0, 0.0, [300.0, 800.0], 2.0),
        None
    );
    assert_eq!(
        cursor_column(bounds, 1_000.0, 2.0, [300.0, 800.0], 0.0),
        None
    );
    assert_eq!(
        cursor_column(bounds, f32::NAN, 2.0, [300.0, 800.0], 2.0),
        None
    );
}

/// The candle lookup's time shares the column's validation and sits at the cursor, not at an edge.
#[test]
fn cursor_time_is_the_exact_cursor_and_shares_the_column_guards() {
    let bounds = [100.0, 50.0, 600.0, 800.0];
    assert_eq!(
        cursor_time(bounds, 1_000.0, 2.0, [300.0, 300.0]),
        Some(1100.0)
    );
    // The left edge clamps the column's own start, but never the time under the pointer.
    assert_eq!(
        cursor_time(bounds, 1_000.0, 2.0, [100.0, 300.0]),
        Some(1000.0)
    );
    assert_eq!(
        cursor_column(bounds, 1_000.0, 2.0, [100.0, 300.0], 2.0),
        Some((1000.0, 1003.0))
    );
    for cursor in [[99.0, 300.0], [300.0, 851.0]] {
        assert_eq!(cursor_time(bounds, 1_000.0, 2.0, cursor), None);
    }
    assert_eq!(
        cursor_time([100.0, 50.0, 0.0, 800.0], 1_000.0, 2.0, [100.0, 300.0]),
        None
    );
}

/// Tick rows follow the native band's opacity; the candle figure beside them does not.
#[test]
fn tick_rows_follow_the_native_band_opacity() {
    assert!(tick_ranges_visible(0.5));
    assert!(tick_ranges_visible(1.0));
    for alpha in [0.0, -1.0, f32::NAN] {
        assert!(!tick_ranges_visible(alpha));
    }
}

/// A price move changes notional even when base quantity stays fixed.
#[test]
fn readout_values_are_quote_notional_and_invalid_values_are_omitted() {
    assert_eq!(quote_notional(25.0, 4.0), 100.0);
    assert_eq!(quote_notional(50.0, 4.0), 200.0);
    for (price, qty) in [
        (f32::NAN, 1.0),
        (1.0, f32::INFINITY),
        (-2.0, 1.0),
        (2.0, -1.0),
        (f32::MAX, 2.0),
        (0.0, 3.0),
    ] {
        assert_eq!(quote_notional(price, qty), 0.0);
    }
}

/// Colliding timestamps preserve all prints and both sides, without candle-style summation.
#[test]
fn overlapping_ticks_report_individual_range_and_count() {
    let got = nearby_ticks(
        [
            (10.0, 0, 20.0),
            (10.0, 0, 80.0),
            (10.0, 0, 20.0),
            (10.0, 1, 7.0),
            (10.0, 2, 999.0),
            (30.0, 0, 500.0),
            (10.0, 1, f32::NAN),
            (f32::NAN, 0, 100.0),
        ],
        9.0,
        11.0,
    );
    assert_eq!(
        got,
        [
            Some(TickVolumeRange {
                count: 3,
                min: 20.0,
                max: 80.0
            }),
            Some(TickVolumeRange {
                count: 1,
                min: 7.0,
                max: 7.0
            })
        ]
    );
    assert_eq!(nearby_ticks([], 9.0, 11.0), [None, None]);
    assert_eq!(nearby_ticks([(0.0, 0, 10.0)], f32::NAN, 1.0), [None, None]);
}

/// Text precedes GPU preparation, so it must observe pending resets and capacity eviction.
#[test]
fn pending_uploads_replace_and_evict_the_same_rows_as_the_ring() {
    let resident = [40, 20, 30]; // chronological order: 20, 30, 40
    assert_eq!(
        pending_ring(&resident, 1, 3, 3, None, &[50])
            .copied()
            .collect::<Vec<_>>(),
        [30, 40, 50]
    );
    assert_eq!(
        pending_ring(&resident, 1, 3, 3, Some(&[1, 2, 3, 4]), &[5])
            .copied()
            .collect::<Vec<_>>(),
        [3, 4, 5]
    );
    assert_eq!(
        pending_ring(&resident, 1, 3, 3, None, &[1, 2, 3, 4])
            .copied()
            .collect::<Vec<_>>(),
        [2, 3, 4]
    );
    assert_eq!(pending_ring(&resident, 1, 3, 3, Some(&[]), &[]).count(), 0);
    assert_eq!(pending_ring(&resident, 0, 0, 0, None, &[]).count(), 0);
}
