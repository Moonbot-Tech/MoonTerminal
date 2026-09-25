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

/// Deterministic LCG so the dense fixtures are reproducible without a rand dependency.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n
    }
}

/// Identity-scaled bake: integer times and prices land on exact texels, so the oracle below
/// keys rows as `col = t`, `row = 600 - p` without re-running the shader arithmetic.
fn lod_geometry() -> super::BakeColumns {
    super::BakeColumns {
        time0: 0.0,
        time_to_px: 1.0,
        price0: 0.0,
        price_to_px: 1.0,
        height: 600.0,
        width_px: 2400,
        volume_alpha: 0.3,
        marker_half: 3.5,
        buy_inv: 0.01,
        sell_inv: 0.01,
    }
}

/// `tick_volume.rs:lod_applies` / `reduce_crosses` / `reduce_volume`: a threshold that never
/// fires (`rows_in_span > usize::MAX / 2`) or a cull filter that keeps everything would make a
/// dense 98k-row bake instance every cross again, or draw rows the shader culls; a tier that
/// drops a column's extreme rows or its tallest bar silently erases edge crosses and spikes.
#[test]
fn dense_bake_is_thinned_per_pixel_column_and_keeps_the_extremes() {
    use std::collections::{HashMap, HashSet};
    const ROWS: usize = 98_304;
    let g = lod_geometry();
    // Boundary pair on the density threshold, and the dense case the goal is about.
    assert!(
        !super::lod_applies(2 * 2400, 2400),
        "rows <= 2*width stays unreduced"
    );
    assert!(
        super::lod_applies(2 * 2400 + 1, 2400),
        "one row past 2*width reduces"
    );
    assert!(
        super::lod_applies(ROWS, 2400),
        "98304 rows over 2400 px must reduce"
    );

    let mut rng = Lcg(0x6100_d15e);
    let mut rows = Vec::with_capacity(ROWS);
    // Crosses land in 303 columns (-3..300) over 600 rows; volume qty <= 50 of a 100 scale.
    for slot in 0..ROWS - 1000 {
        let t = rng.below(303) as f32 - 3.0;
        let p = rng.below(601) as f32;
        let side = rng.below(3) as u32;
        let qty = 1.0 + rng.below(50) as f32;
        rows.push((slot as u32, t, p, side, qty));
    }
    // Off-screen rows the shader culls (far right and far below the bake).
    for k in 0..1000u32 {
        let slot = (ROWS - 1000) as u32 + k;
        let (t, p) = if k % 2 == 0 {
            (3000.0, 300.0)
        } else {
            (100.0, -500.0)
        };
        rows.push((slot, t, p, 0, 5.0));
    }
    // One planted full-scale bar per (column, buy/sell), strictly taller than every other bar.
    let mut planted = Vec::new();
    for col in 0..300u32 {
        for side in 0..2u32 {
            let at = rng.below(ROWS as u64 - 1000) as usize;
            rows[at] = (rows[at].0, col as f32, 300.0, side, 100.0);
            planted.push(rows[at].0);
        }
    }
    let mut pick = super::LodPick::default();
    super::reduce_crosses(rows.iter().copied(), &g, &mut pick);
    let cross: Vec<u32> = pick.cross.clone();
    super::reduce_volume(rows.iter().copied(), &g, &mut pick);
    let volume: Vec<u32> = pick.volume.clone();
    println!(
        "[G1] dense bake {ROWS} rows width 2400: cross_instances old={ROWS} new={} \
         volume_instances old={ROWS} new={}",
        cross.len(),
        volume.len()
    );

    let v = super::lod_volume_keep(0.3);
    let cols = 303;
    assert!(cross.len() <= cols * 3 * (super::LOD_MAX_ROWS + 2));
    assert!(
        cross.len() * 2 < ROWS,
        "crosses must shrink well below the input"
    );
    assert!(volume.len() <= cols * (v * 73 + 2));
    assert!(
        volume.len() * 2 < ROWS,
        "bars must shrink well below the input"
    );

    // Cull bound: nothing the shader would drop (|col| or |row| past max(8, half+1)) survives.
    let by_slot: HashMap<u32, (u32, f32, f32, u32, f32)> = rows.iter().map(|r| (r.0, *r)).collect();
    for s in &cross {
        let (_, t, p, _, _) = by_slot[s];
        let (col, row) = (t, 600.0 - p);
        assert!(
            (-8.0..=2408.0).contains(&col) && (-8.0..=608.0).contains(&row),
            "culled cross slot {s} kept"
        );
    }
    // Per (column, side class) the extreme rows among kept crosses equal brute force over the
    // crosses the draw shows: the last row per texel, whatever its side.
    let mut top: HashMap<(i64, i64), (u32, f32, f32, u32, f32)> = HashMap::new();
    for r in &rows[..ROWS - 1000] {
        top.insert((r.1 as i64, r.2 as i64), *r);
    }
    let mut want: HashMap<(i64, u32), (i64, i64)> = HashMap::new();
    for &(_, t, p, side, _) in top.values() {
        let e = want.entry((t as i64, side)).or_insert((i64::MAX, i64::MIN));
        let row = 600 - p as i64;
        e.0 = e.0.min(row);
        e.1 = e.1.max(row);
    }
    let mut got: HashMap<(i64, u32), (i64, i64)> = HashMap::new();
    for s in &cross {
        let (_, t, p, side, _) = by_slot[s];
        let e = got.entry((t as i64, side)).or_insert((i64::MAX, i64::MIN));
        let row = 600 - p as i64;
        e.0 = e.0.min(row);
        e.1 = e.1.max(row);
    }
    assert_eq!(got, want, "a column lost its top or bottom cross");
    // Every planted tallest bar is drawn.
    let vol_set: HashSet<u32> = volume.iter().copied().collect();
    for s in &planted {
        assert!(vol_set.contains(s), "tallest bar slot {s} dropped");
    }
}

/// `tick_volume.rs:reduce_crosses` tier A: on <= 32 rows per column the kept list is exactly
/// the last row per (column, row) in original order; keying the off-edge column -3 onto 0 (a
/// clamp) would merge it and erase the edge cross.
#[test]
fn sparse_columns_keep_the_last_row_per_texel_in_draw_order() {
    let g = lod_geometry();
    let mut rng = Lcg(42);
    let mut rows = Vec::new();
    for slot in 0..600u32 {
        let t = rng.below(23) as f32 - 3.0;
        let p = 290.0 + rng.below(20) as f32;
        rows.push((slot, t, p, rng.below(2) as u32, 1.0));
    }
    rows.push((600, -3.0, 5.0, 0, 1.0));
    rows.push((601, 0.0, 5.0, 0, 1.0));
    let mut last = std::collections::HashMap::new();
    for &(slot, t, p, _, _) in &rows {
        last.insert((t as i64, p as i64), slot);
    }
    let mut want: Vec<u32> = last.into_values().collect();
    want.sort_unstable();
    let mut pick = super::LodPick::default();
    super::reduce_crosses(rows.iter().copied(), &g, &mut pick);
    assert_eq!(pick.cross, want);
    assert!(pick.cross.contains(&600) && pick.cross.contains(&601));
}

/// Row key from the comparison-sort bake at 4f7bd250, kept beside `LodRow` so the oracle
/// does not read the counting-sort scratch.
#[derive(Clone, Copy)]
struct OracleRow {
    slot: u32,
    col: i64,
    row: i64,
    side: u32,
    qty: f32,
}

/// Scratch for the pre-change `key_rows` / `reduce_crosses` / `reduce_volume` bodies.
#[derive(Default)]
struct OraclePick {
    cross: Vec<u32>,
    volume: Vec<u32>,
    rows: Vec<OracleRow>,
    order: Vec<u32>,
    kept: Vec<u32>,
    keep: Vec<bool>,
    covering: Vec<u32>,
}

/// `key_rows` as it was at 4f7bd250: shader texel keys, cull left to `drawn`.
///
/// Returns nothing. `out.rows` is replaced with the rows `drawn` kept.
fn oracle_key_rows(
    rows: impl IntoIterator<Item = (u32, f32, f32, u32, f32)>,
    g: &super::BakeColumns,
    out: &mut OraclePick,
    drawn: impl Fn(f32, f32, f32, u32, f32) -> bool,
) {
    out.rows.clear();
    for (slot, t, p, side, qty) in rows {
        let sx = 0.0 + (t - g.time0) * g.time_to_px;
        let col = sx.round_ties_even();
        let row = ((0.0 + g.height) - (p - g.price0) * g.price_to_px).round_ties_even();
        if !drawn(sx, col, row, side, qty) {
            continue;
        }
        out.rows.push(OracleRow {
            slot,
            col: col as i64,
            row: row as i64,
            side,
            qty,
        });
    }
}

/// `reduce_crosses` as it was at 4f7bd250 (comparison sorts, not counting sorts).
///
/// `out.cross` becomes the kept slots in ascending input order.
fn oracle_reduce_crosses(
    rows: impl IntoIterator<Item = (u32, f32, f32, u32, f32)>,
    g: &super::BakeColumns,
    out: &mut OraclePick,
) {
    out.cross.clear();
    let cull = g.marker_half.max(8.0).max(g.marker_half + 1.0);
    let (w, h) = (g.width_px as f32, g.height);
    oracle_key_rows(rows, g, out, |_, col, row, _, _| {
        col.is_finite()
            && row.is_finite()
            && col >= -cull
            && col <= w + cull
            && row >= -cull
            && row <= h + cull
    });
    let OraclePick {
        cross,
        rows,
        order,
        kept,
        keep,
        ..
    } = out;
    let n = rows.len();
    keep.clear();
    keep.resize(n, false);
    order.clear();
    order.extend(0..n as u32);
    order.sort_unstable_by_key(|&k| {
        let r = &rows[k as usize];
        (r.col, r.row, k)
    });
    kept.clear();
    for (i, &k) in order.iter().enumerate() {
        let r = &rows[k as usize];
        let last_of_texel = order.get(i + 1).is_none_or(|&next| {
            let q = &rows[next as usize];
            (q.col, q.row) != (r.col, r.row)
        });
        if last_of_texel {
            kept.push(k);
        }
    }
    kept.sort_unstable_by_key(|&k| {
        let r = &rows[k as usize];
        (r.col, r.side.min(2), r.row, k)
    });
    let mut col_start = 0;
    while col_start < kept.len() {
        let col = rows[kept[col_start] as usize].col;
        let col_end = col_start
            + kept[col_start..]
                .iter()
                .take_while(|&&k| rows[k as usize].col == col)
                .count();
        if col_end - col_start <= super::LOD_MAX_ROWS {
            for &k in &kept[col_start..col_end] {
                keep[k as usize] = true;
            }
        } else {
            let mut side_start = col_start;
            while side_start < col_end {
                let class = rows[kept[side_start] as usize].side.min(2);
                let side_end = side_start
                    + kept[side_start..col_end]
                        .iter()
                        .take_while(|&&k| rows[k as usize].side.min(2) == class)
                        .count();
                let side_rows = &kept[side_start..side_end];
                let stride = side_rows.len().div_ceil(super::LOD_MAX_ROWS).max(1);
                for (pos, &k) in side_rows.iter().enumerate() {
                    if pos % stride == 0 {
                        keep[k as usize] = true;
                    }
                }
                keep[side_rows[side_rows.len() - 1] as usize] = true;
                side_start = side_end;
            }
        }
        col_start = col_end;
    }
    cross.extend((0..n).filter(|&k| keep[k]).map(|k| rows[k].slot));
}

/// `reduce_volume` as it was at 4f7bd250, including the full cover sum.
///
/// `out.volume` becomes the kept slots in ascending input order.
fn oracle_reduce_volume(
    rows: impl IntoIterator<Item = (u32, f32, f32, u32, f32)>,
    g: &super::BakeColumns,
    out: &mut OraclePick,
) {
    out.volume.clear();
    let w = g.width_px as f32;
    oracle_key_rows(rows, g, out, |sx, _, _, side, qty| {
        sx >= -2.0 && sx <= w + 2.0 && qty > 0.0 && side < 2
    });
    let OraclePick {
        volume,
        rows,
        order,
        kept,
        covering,
        ..
    } = out;
    let n = rows.len();
    let v = super::lod_volume_keep(g.volume_alpha) as u32;
    let band_h = (g.height * 0.18).min(72.0);
    let height_px = |r: &OracleRow| {
        let inv = if r.side == 0 { g.buy_inv } else { g.sell_inv };
        let norm = (r.qty * inv).clamp(0.0, 1.0);
        let h = (norm.sqrt() * band_h).max(1.0).ceil();
        if h.is_finite() {
            (h as usize).min(super::LOD_BAR_MAX_PX)
        } else {
            super::LOD_BAR_MAX_PX
        }
    };
    order.clear();
    order.extend(0..n as u32);
    order.sort_unstable_by_key(|&k| (rows[k as usize].col, k));
    kept.clear();
    let mut start = 0;
    while start < order.len() {
        let col = rows[order[start] as usize].col;
        let end = start
            + order[start..]
                .iter()
                .take_while(|&&k| rows[k as usize].col == col)
                .count();
        let group = &order[start..end];
        let mut tallest = [u32::MAX; 3];
        for &k in group {
            let r = &rows[k as usize];
            let best = &mut tallest[r.side.min(2) as usize];
            if *best == u32::MAX || height_px(r) >= height_px(&rows[*best as usize]) {
                *best = k;
            }
        }
        covering.clear();
        covering.resize(super::LOD_BAR_MAX_PX + 1, 0);
        for &k in group.iter().rev() {
            let hp = height_px(&rows[k as usize]);
            let covered: u32 = covering[hp..].iter().sum();
            if tallest.contains(&k) || covered < v {
                covering[hp] += 1;
                kept.push(k);
            }
        }
        start = end;
    }
    kept.sort_unstable();
    volume.extend(kept.iter().map(|&k| rows[k as usize].slot));
}

/// Compare both kept lists to the 4f7bd250 oracle. Cross is checked first.
fn assert_matches_presort_oracle(
    label: &str,
    rows: &[(u32, f32, f32, u32, f32)],
    g: &super::BakeColumns,
) {
    let mut oracle = OraclePick::default();
    oracle_reduce_crosses(rows.iter().copied(), g, &mut oracle);
    let want_cross = std::mem::take(&mut oracle.cross);
    oracle_reduce_volume(rows.iter().copied(), g, &mut oracle);
    let want_volume = std::mem::take(&mut oracle.volume);
    assert!(
        !want_cross.is_empty() || !want_volume.is_empty(),
        "{label} oracle kept nothing"
    );
    let mut pick = super::LodPick::default();
    super::reduce_crosses(rows.iter().copied(), g, &mut pick);
    assert_eq!(pick.cross, want_cross, "{label} cross");
    super::reduce_volume(rows.iter().copied(), g, &mut pick);
    assert_eq!(pick.volume, want_volume, "{label} volume");
}

/// 98_304 trades over 3600s. Times and prices are integer so an identity bake is exact.
fn dense_wheel_rows() -> Vec<(u32, f32, f32, u32, f32)> {
    const ROWS: usize = 98_304;
    let mut rng = Lcg(0x6100_d15e);
    let mut rows = Vec::with_capacity(ROWS);
    for slot in 0..ROWS {
        let t = rng.below(3600) as f32;
        let p = rng.below(600) as f32;
        let side = rng.below(3) as u32;
        let qty = 1.0 + rng.below(50) as f32;
        rows.push((slot as u32, t, p, side, qty));
    }
    rows
}

/// 2300x900 bake. `time_to_px == 0` puts every finite time on column 0.
/// Prices `0..600` map onto the height.
fn window_bake(time_to_px: f32, alpha: f32, marker_half: f32) -> super::BakeColumns {
    super::BakeColumns {
        time0: 0.0,
        time_to_px,
        price0: 0.0,
        price_to_px: 900.0 / 600.0,
        height: 900.0,
        width_px: 2300,
        volume_alpha: alpha,
        marker_half,
        buy_inv: 0.02,
        sell_inv: 0.02,
    }
}

/// Identity bake (`col = t`, `row = 600 - p`) of `width` texels.
fn identity_bake(width: u32, alpha: f32, marker_half: f32) -> super::BakeColumns {
    super::BakeColumns {
        time0: 0.0,
        time_to_px: 1.0,
        price0: 0.0,
        price_to_px: 1.0,
        height: 600.0,
        width_px: width,
        volume_alpha: alpha,
        marker_half,
        buy_inv: 0.02,
        sell_inv: 0.02,
    }
}

/// Last drawn slot per texel, ignoring the per-side sample. Valid only at <= 32 rows.
fn identity_last_slots(rows: &[(u32, f32, f32, u32, f32)], g: &super::BakeColumns) -> Vec<u32> {
    let cull = g.marker_half.max(8.0).max(g.marker_half + 1.0);
    let (w, h) = (g.width_px as f32, g.height);
    let mut last: std::collections::BTreeMap<(i64, i64), (usize, u32)> =
        std::collections::BTreeMap::new();
    for (index, &(slot, t, p, _, _)) in rows.iter().enumerate() {
        let col = ((t - g.time0) * g.time_to_px).round_ties_even();
        let row = (g.height - (p - g.price0) * g.price_to_px).round_ties_even();
        if col.is_finite()
            && row.is_finite()
            && (-cull..=w + cull).contains(&col)
            && (-cull..=h + cull).contains(&row)
        {
            last.insert((col as i64, row as i64), (index, slot));
        }
    }
    let mut winners: Vec<(usize, u32)> = last.into_values().collect();
    winners.sort_unstable();
    winners.into_iter().map(|(_, slot)| slot).collect()
}

/// Copy `rows` and append the four cull corners, which are exact on an identity bake.
fn with_cull_corners(
    rows: &[(u32, f32, f32, u32, f32)],
    g: &super::BakeColumns,
) -> Vec<(u32, f32, f32, u32, f32)> {
    let cull = g.marker_half.max(8.0).max(g.marker_half + 1.0);
    let w = g.width_px as f32;
    let spots = [
        (-cull, -cull),
        (-cull, g.height + cull),
        (w + cull, -cull),
        (w + cull, g.height + cull),
    ];
    let mut out = rows.to_vec();
    for (col, row) in spots {
        let slot = out.len() as u32;
        let t = g.time0 + col / g.time_to_px;
        let p = g.price0 + (g.height - row) / g.price_to_px;
        out.push((slot, t, p, 0, 3.0));
    }
    out
}

/// `tick_volume.rs:counting_sort_by`: scattering with `order.clone().iter().rev()`
/// reverses equal keys. Volume sorts by that function once, so the cover walk keeps
/// the early bars in a column instead of the later ones and a wheel zoom draws the
/// wrong bars. Crosses sort twice (row, then column), so this edit does not change
/// which cross is last on a texel.
#[test]
fn counting_sort_presort_oracle_on_dense_windows() {
    let g = identity_bake(2400, 0.3, 3.5);
    let mut dup = Vec::with_capacity(530);
    for slot in 0..500u32 {
        dup.push((slot, 10.0, 20.0, slot % 3, 2.0));
    }
    for slot in 500..530u32 {
        dup.push((slot, 11.0 + (slot - 500) as f32, 21.0, 0, 2.0));
    }
    let mut oracle = OraclePick::default();
    oracle_reduce_crosses(dup.iter().copied(), &g, &mut oracle);
    assert_eq!(
        oracle.cross,
        identity_last_slots(&dup, &g),
        "duplicate texel oracle is the last drawn slot"
    );
    assert!(oracle.cross.contains(&499) && !oracle.cross.contains(&0));
    assert_matches_presort_oracle("duplicate texels", &dup, &g);

    let rows = dense_wheel_rows();
    assert_eq!(rows.len(), 98_304);
    assert_matches_presort_oracle(
        "whole window",
        &rows,
        &window_bake(2300.0 / 3600.0, 0.3, 3.5),
    );
    assert_matches_presort_oracle(
        "half window",
        &rows,
        &window_bake(2300.0 / 1800.0, 0.3, 3.5),
    );
    assert_matches_presort_oracle(
        "five minute window",
        &rows,
        &window_bake(2300.0 / 300.0, 0.3, 3.5),
    );
    assert_matches_presort_oracle("one column", &rows, &window_bake(0.0, 0.3, 3.5));

    let edge = identity_bake(2400, 0.3, 3.5);
    let edged = with_cull_corners(&rows[..4_000], &edge);
    assert_matches_presort_oracle("cull edges", &edged, &edge);
    let wide = identity_bake(2400, 0.3, 40.0);
    let wide_rows = with_cull_corners(&rows[..4_000], &wide);
    assert_matches_presort_oracle("large marker half", &wide_rows, &wide);
}

/// Seeded rows: non-finite values, negative columns, side classes past 2, empty qty.
fn edge_input_rows() -> Vec<(u32, f32, f32, u32, f32)> {
    let mut rng = Lcg(0xB2E4_6E01);
    let mut rows = Vec::new();
    for i in 0..80u32 {
        rows.push((i, 10.0, i as f32, 0, 4.0));
    }
    for _ in 0..240 {
        let slot = rows.len() as u32;
        let t = rng.below(40) as f32 - 4.0;
        let p = rng.below(80) as f32;
        let side = rng.below(6) as u32;
        let qty = if rng.below(7) == 0 {
            0.0
        } else {
            1.0 + rng.below(20) as f32
        };
        rows.push((slot, t, p, side, qty));
    }
    rows
}

/// Same rows plus NaN and infinite time and price. A kept non-finite price forces the fallback.
fn with_nonfinite(rows: &[(u32, f32, f32, u32, f32)]) -> Vec<(u32, f32, f32, u32, f32)> {
    let specs = [
        (f32::NAN, 12.0),
        (f32::INFINITY, 12.0),
        (f32::NEG_INFINITY, 12.0),
        (12.0, f32::NAN),
        (12.0, f32::INFINITY),
        (12.0, f32::NEG_INFINITY),
    ];
    let mut out = rows.to_vec();
    for (t, p) in specs {
        let slot = out.len() as u32;
        out.push((slot, t, p, 0, 5.0));
    }
    out
}

/// `stack` equal bars at column 0 plus one bar at `far`, so the column span is `far + 1`.
fn span_rows(far: f32, stack: u32) -> Vec<(u32, f32, f32, u32, f32)> {
    let mut rows = Vec::with_capacity(stack as usize + 1);
    for i in 0..stack {
        rows.push((i, 0.0, (i % 80) as f32, 0, 4.0));
    }
    rows.push((stack, far, 0.0, 0, 4.0));
    rows
}

/// `tick_volume.rs:cover_reached`: `covered > keep` instead of `covered >= keep` keeps
/// bars the 8-bit composite had already hidden. A wheel zoom draws those extra bars.
#[test]
fn cover_reached_presort_oracle_on_edge_inputs() {
    let rows = edge_input_rows();
    let keep_tight = super::lod_volume_keep(0.3);
    let keep_loose = super::lod_volume_keep(0.05);
    let keep_opaque = super::lod_volume_keep(1.0);
    assert!(keep_loose > keep_tight && keep_tight > keep_opaque);
    let g = identity_bake(80, 0.3, 3.5);
    let mut oracle = OraclePick::default();
    oracle_reduce_volume(rows.iter().copied(), &g, &mut oracle);
    let stacked = rows.iter().filter(|r| r.1 == 10.0 && r.4 == 4.0).count();
    let kept = rows
        .iter()
        .filter(|r| r.1 == 10.0 && r.4 == 4.0 && oracle.volume.contains(&r.0))
        .count();
    assert!(
        kept > 0 && kept < stacked,
        "cover count dropped no stacked bar"
    );
    assert_matches_presort_oracle("alpha 0.3", &rows, &g);
    assert_matches_presort_oracle("alpha 0.05", &rows, &identity_bake(80, 0.05, 3.5));
    assert_matches_presort_oracle("alpha 1", &rows, &identity_bake(80, 1.0, 3.5));
    assert_matches_presort_oracle(
        "nonfinite fallback",
        &with_nonfinite(&rows),
        &identity_bake(80, 0.3, 3.5),
    );

    let stack = 80u32;
    let n = stack as usize + 1;
    let budget = n * 4 + 4096;
    let under = span_rows((budget - 1) as f32, stack);
    let over = span_rows(budget as f32, stack);
    assert_matches_presort_oracle(
        "bucket budget",
        &under,
        &identity_bake((budget - 1) as u32, 0.3, 3.5),
    );
    assert_matches_presort_oracle(
        "past bucket budget",
        &over,
        &identity_bake(budget as u32, 0.3, 3.5),
    );
}

/// Capacities of the scratch `reduce_crosses` / `reduce_volume` reuse across bakes.
fn scratch_caps(pick: &super::LodPick) -> [usize; 8] {
    [
        pick.rows.capacity(),
        pick.order.capacity(),
        pick.kept.capacity(),
        pick.keep.capacity(),
        pick.covering.capacity(),
        pick.counts.capacity(),
        pick.scratch.capacity(),
        pick.heights.capacity(),
    ]
}

/// `tick_volume.rs:counting_sort_by`: a second bake of the same row count must not grow
/// `LodPick` scratch. Growth here allocates on every wheel zoom.
#[test]
fn second_bake_keeps_lodpick_scratch_capacity() {
    let rows = dense_wheel_rows();
    let g = window_bake(2300.0 / 3600.0, 0.3, 3.5);
    let mut pick = super::LodPick::default();
    super::reduce_crosses(rows.iter().copied(), &g, &mut pick);
    super::reduce_volume(rows.iter().copied(), &g, &mut pick);
    let caps = scratch_caps(&pick);
    super::reduce_crosses(rows.iter().copied(), &g, &mut pick);
    super::reduce_volume(rows.iter().copied(), &g, &mut pick);
    assert_eq!(scratch_caps(&pick), caps, "scratch grew on the second bake");
}

/// Ignored timing probe: median milliseconds, ASCII only, not an assertion.
#[test]
#[ignore]
fn ignored_wheel_bake_reduce_median_ms() {
    let rows = dense_wheel_rows();
    for window in [3600.0f32, 1800.0, 300.0] {
        let g = window_bake(2300.0 / window, 0.3, 3.5);
        let mut pick = super::LodPick::default();
        let mut cross_ms = Vec::with_capacity(20);
        let mut volume_ms = Vec::with_capacity(20);
        for _ in 0..20 {
            let started = std::time::Instant::now();
            super::reduce_crosses(rows.iter().copied(), &g, &mut pick);
            cross_ms.push(started.elapsed().as_secs_f64() * 1000.0);
            let started = std::time::Instant::now();
            super::reduce_volume(rows.iter().copied(), &g, &mut pick);
            volume_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        cross_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        volume_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let cross = (cross_ms[9] + cross_ms[10]) / 2.0;
        let volume = (volume_ms[9] + volume_ms[10]) / 2.0;
        println!(
            "[OK] window {window}s median reduce_crosses {cross:.3} ms reduce_volume {volume:.3} ms"
        );
    }
}
