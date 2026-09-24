//! Regression cases for quote units, overlapping ticks, and pending native uploads.

use super::{
    TickTimeOrder, TickVolumeRange, cursor_column, cursor_time, nearby_ticks, pending_ring,
    pending_ring_at, quote_notional, tick_bake_span, tick_ranges_visible, tick_slot_runs,
    tick_time_range, tick_touches_bake,
};

/// A wrong partition loses equal-time prints; rotating the ring must not change picks or draw order.
#[test]
fn time_lookup_matches_linear_oracle_across_ring_layouts() {
    for capacity in 1..12 {
        for count in 0..=capacity {
            for head in 0..capacity {
                let mut slots = vec![0.0; capacity];
                let origin = if count == capacity { head } else { 0 };
                for i in 0..count {
                    slots[(origin + i) % capacity] = (i / 2) as f32;
                }
                let rows = pending_ring(&slots, head, count, capacity, None, &[]);
                for from in -1..8 {
                    let to = from + 1;
                    let range = tick_time_range(rows.len(), 0.0, from as f64, to as f64, |i| {
                        *rows.clone().nth(i).unwrap()
                    });
                    let expected: Vec<_> = rows
                        .clone()
                        .filter(|&&t| t >= from as f32 && t <= to as f32)
                        .copied()
                        .collect();
                    assert_eq!(
                        rows.clone()
                            .skip(range.start)
                            .take(range.len())
                            .copied()
                            .collect::<Vec<_>>(),
                        expected
                    );
                    let runs = tick_slot_runs(range, head, count, capacity);
                    let actual: Vec<_> = runs
                        .into_iter()
                        .flat_map(|(start, len)| (start..start + len).map(|i| (i, slots[i])))
                        .collect();
                    let expected_slots: Vec<_> = slots
                        .iter()
                        .enumerate()
                        .take(count)
                        .filter(|(_, t)| **t >= from as f32 && **t <= to as f32)
                        .map(|(i, &t)| (i, t))
                        .collect();
                    assert_eq!(actual, expected_slots);
                }
            }
        }
    }
}

/// Losing the non-finite sentinel lets binary search omit previously drawable rows.
#[test]
fn nonfinite_times_force_conservative_lookup() {
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let rows = [1.0, invalid, 2.0];
        let mut order = TickTimeOrder::default();
        order.extend(rows);
        order.extend([100.0, 101.0]);
        assert_eq!(
            tick_time_range(rows.len(), order.max_lateness(), 2.0, 2.0, |i| rows[i]),
            0..rows.len()
        );
    }
}

/// Using the last time rather than the prefix maximum misses cumulative liquidation lateness.
/// A full-walk disorder fallback also fails the bounded candidate/probe budgets.
#[test]
fn interleaved_late_liquidations_match_linear_windows() {
    let mut rows = Vec::new();
    let mut order = TickTimeOrder::default();
    let mut seed = 0x71c5_u32;
    for batch in 0..400 {
        let time = (batch * 10) as f32;
        let trades = [(time, 0), (time, 1), (time + 2.0, 0)];
        order.extend(trades.map(|row| row.0));
        rows.extend(trades);
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let liquidations = [(time - (seed % 12) as f32, 2), (time - 15.0, 2)];
        order.extend(liquidations.map(|row| row.0));
        rows.extend(liquidations);
    }
    for from in (-20..4020).step_by(3) {
        for width in [0, 1, 7, 23] {
            let to = (from + width) as f64;
            let from = from as f64;
            let probes = std::cell::Cell::new(0);
            let range = tick_time_range(rows.len(), order.max_lateness(), from, to, |i| {
                probes.set(probes.get() + 1);
                rows[i].0
            });
            let matches = |row: &&(f32, u32)| f64::from(row.0) >= from && f64::from(row.0) <= to;
            let expected: Vec<_> = rows.iter().filter(matches).copied().collect();
            let actual: Vec<_> = rows[range.clone()]
                .iter()
                .filter(matches)
                .copied()
                .collect();
            assert_eq!(actual, expected, "window {from}..={to}");
            assert!(probes.get() <= 22);
            assert!(range.len() <= 50, "narrow windows must not walk the ring");
        }
    }
}

/// Shrinking L on newer batches loses late rows still resident; equal times need no widening.
#[test]
fn lateness_survives_newer_batches_and_handles_finite_extremes() {
    let mut order = TickTimeOrder::default();
    order.extend([1.0, 1.0, 2.0]);
    assert_eq!(order.max_lateness(), 0.0);
    order.extend([1.5, 0.0]);
    order.extend([3.0, 4.0]);
    assert_eq!(order.max_lateness(), 2.0);
    let extremes = [f32::MAX, -f32::MAX];
    order.extend(extremes);
    assert!(order.max_lateness().is_finite());
    assert_eq!(
        tick_time_range(2, order.max_lateness(), 0.0, f64::from(f32::MAX), |i| {
            extremes[i]
        }),
        0..2
    );
}

/// Replacing the binary search with a linear pass exceeds the probe budget on Max history.
#[test]
fn narrow_lookup_has_logarithmic_probe_cost() {
    let probes = std::cell::Cell::new(0);
    let range = tick_time_range(98_000, 0.0, 50_000.0, 50_002.0, |i| {
        probes.set(probes.get() + 1);
        i as f32
    });
    assert_eq!(range, 50_000..50_003);
    assert!(probes.get() <= 34);
}

/// Omitting marker/rounding margins leaves stale edge pixels after eviction; old ticks stay cheap.
#[test]
fn eviction_checks_both_bake_edges_and_marker_margin() {
    let span = tick_bake_span(100.0, 200.0, 2.0, 16.0);
    assert!(!tick_touches_bake(90.0, span));
    for time in [91.0, 100.0, 200.0, 209.0] {
        assert!(tick_touches_bake(time, span));
    }
    assert!(!tick_touches_bake(210.0, span));
    assert!(tick_touches_bake(f32::NAN, span));
    assert!(tick_touches_bake(
        0.0,
        tick_bake_span(100.0, 200.0, 0.0, 3.5)
    ));
}

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

/// Wrong slice/origin/tail arithmetic substitutes rows during wrap, reset, or pending eviction.
#[test]
fn indexed_pending_rows_match_independent_materialized_ring() {
    for capacity in 1..9 {
        for count in 0..=capacity {
            for head in 0..capacity {
                let resident: Vec<_> = (0..capacity).collect();
                for reset_len in 0..=capacity + 2 {
                    let reset: Vec<_> = (100..100 + reset_len).collect();
                    for reset in [None, Some(reset.as_slice())] {
                        for append_len in 0..=capacity + 2 {
                            let append: Vec<_> = (200..200 + append_len).collect();
                            let mut expected = match reset {
                                Some(rows) => rows.to_vec(),
                                None if count == capacity => resident[head..]
                                    .iter()
                                    .chain(&resident[..head])
                                    .copied()
                                    .collect(),
                                None => resident[..count].to_vec(),
                            };
                            expected.extend(&append);
                            let expected = &expected[expected.len().saturating_sub(capacity)..];
                            let actual: Vec<_> = (0..expected.len())
                                .map(|i| {
                                    *pending_ring_at(
                                        &resident, head, count, capacity, reset, &append, i,
                                    )
                                })
                                .collect();
                            assert_eq!(actual, expected);
                            assert_eq!(
                                pending_ring(&resident, head, count, capacity, reset, &append)
                                    .copied()
                                    .collect::<Vec<_>>(),
                                expected
                            );
                        }
                    }
                }
            }
        }
    }
}
