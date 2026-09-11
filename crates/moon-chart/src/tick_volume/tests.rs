//! Regression cases for quote units, overlapping ticks, and pending native uploads.

use super::{TickVolumeRange, cursor_column, nearby_ticks, pending_ring, quote_notional};

/// Hidden layers and off-band cursors have no values; DPI scales the pick column once.
#[test]
fn cursor_column_respects_native_band_and_opacity() {
    let bounds = [100.0, 50.0, 600.0, 800.0]; // band floor 849, ceiling 777 device pixels
    assert_eq!(
        cursor_column(bounds, 1_000.0, 2.0, [300.0, 800.0], 2.0, 0.5),
        Some((1097.0, 1103.0))
    );
    for cursor in [
        [300.0, 776.0],
        [300.0, 850.0],
        [99.0, 800.0],
        [701.0, 800.0],
    ] {
        assert_eq!(cursor_column(bounds, 1_000.0, 2.0, cursor, 2.0, 0.5), None);
    }
    assert_eq!(
        cursor_column(bounds, 1_000.0, 2.0, [300.0, 800.0], 2.0, 0.0),
        None
    );
    assert_eq!(
        cursor_column(bounds, 1_000.0, 0.0, [300.0, 800.0], 2.0, 0.5),
        None
    );
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
