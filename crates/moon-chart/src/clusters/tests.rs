//! Unit tests for the trade prints: folding, the market's own scale and merging on zoom.

use super::*;

/// Frozen pre-change fold used to detect changes in print membership, order or summation.
fn reference_aggregate(trades: &[(f64, f32, f32, bool)], out: &mut Vec<Print>) {
    out.clear();
    let mut open = std::collections::HashMap::new();
    for &(time_ms, price, qty, is_buy) in trades {
        let notional = price * qty;
        if !time_ms.is_finite() || !(notional > 0.0) || !notional.is_finite() {
            continue;
        }
        let Some(key) = price_key(price, Some(0.01)) else {
            continue;
        };
        if let Some(&i) = open.get(&(key, is_buy)) {
            let print: &mut Print = &mut out[i];
            if time_ms >= print.time_ms && time_ms - print.time_ms <= 100.0 {
                if is_buy {
                    print.buy += notional;
                } else {
                    print.sell += notional;
                }
                continue;
            }
        }
        open.insert((key, is_buy), out.len());
        out.push(Print {
            time_ms,
            price,
            key,
            buy: if is_buy { notional } else { 0.0 },
            sell: if is_buy { 0.0 } else { notional },
        });
    }
}

/// Original sorted-percentile oracle, independent of rank selection and cached ordering keys.
fn reference_scale(prints: &[Print], scratch: &mut Vec<f32>) -> Option<PrintScale> {
    scratch.clear();
    scratch.extend(
        prints
            .iter()
            .map(Print::total)
            .filter(|v| v.is_finite() && *v > 0.0),
    );
    if scratch.is_empty() {
        return None;
    }
    scratch.sort_unstable_by(f32::total_cmp);
    let at = |pct: f32| scratch[((pct / 100.0) * (scratch.len() - 1) as f32).round() as usize];
    let small = at(50.0);
    Some(PrintScale {
        small,
        big: at(99.0).max(small),
    })
}

/// Run the original or current pure pipeline over the same tape, retaining all observable outputs.
fn paired_pipeline(
    tape: &[(f64, f32, f32, bool)],
    reference: bool,
) -> (Vec<Print>, Vec<u32>, PrintScale, Vec<PrintDisc>) {
    let (mut prints, mut order, mut scratch, mut discs) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    if reference {
        reference_aggregate(tape, &mut prints);
        order.extend(0..prints.len() as u32);
        order.sort_by(|&a, &b| {
            let (a, b) = (&prints[a as usize], &prints[b as usize]);
            a.key.cmp(&b.key).then(a.time_ms.total_cmp(&b.time_ms))
        });
    } else {
        aggregate_prints(tape.iter().copied(), Some(0.01), 100.0, &mut prints);
        price_time_order(&prints, &mut order);
    }
    let scale = if reference {
        reference_scale(&prints, &mut scratch)
    } else {
        print_scale(&prints, 50, &mut scratch)
    }
    .unwrap();
    merge_overlapping(
        &prints,
        &order,
        0.01,
        |v| print_radius(v, scale, DiscRadii::at(1.0)),
        &mut discs,
    );
    (prints, order, scale, discs)
}

/// Pin exact folded prints, painter order, percentile scale and disc pixels to the original path.
#[test]
fn bubble_pipeline_matches_original_outputs() {
    let tape: Vec<_> = (0..600)
        .map(|i| {
            (
                i as f64,
                10.0 + ((i * 73) % 17) as f32 * 0.01,
                1.0 + (i % 37) as f32,
                i % 3 == 0,
            )
        })
        .collect();
    assert_eq!(paired_pipeline(&tape, false), paired_pipeline(&tape, true));
}

/// Counterbalance old/new runs to separate algorithm gains from concurrent machine load.
#[test]
#[ignore = "manual heavy-user paired timing probe"]
fn bench_goal_b_clusters_paired() {
    use std::{hint::black_box, time::Instant};
    let tape: Vec<_> = (0..100_000)
        .map(|i| {
            (
                i as f64 * 10.0,
                10.0 + ((i * 73) % 1000) as f32 * 0.01,
                1.0 + (i % 37) as f32,
                i % 2 == 0,
            )
        })
        .collect();
    assert_eq!(paired_pipeline(&tape, false), paired_pipeline(&tape, true));
    let mut elapsed = [0u128; 2];
    for repetition in 0..20 {
        for arm in [repetition % 2, 1 - repetition % 2] {
            let start = Instant::now();
            black_box(paired_pipeline(black_box(&tape), arm == 0));
            elapsed[arm] += start.elapsed().as_nanos();
        }
    }
    println!(
        "goal_b_bubbles_paired_original: {} ns/iter",
        elapsed[0] / 20
    );
    println!("goal_b_bubbles_paired_current: {} ns/iter", elapsed[1] / 20);
}

/// Preserve tape ties and the full floating-point ordering when cached sort keys change.
#[test]
fn cached_print_order_matches_stable_price_time_comparison() {
    let times = [
        f64::NAN,
        -0.0,
        0.0,
        f64::NEG_INFINITY,
        f64::INFINITY,
        -7.0,
        7.0,
        f64::from_bits(0xfff8_0000_0000_0001),
    ];
    let prints: Vec<_> = (0..257)
        .map(|i| print(times[(i * 7) % times.len()], (i as i64 * 73) % 11 - 5, 1.0))
        .collect();
    let mut expected: Vec<_> = (0..prints.len() as u32).collect();
    expected.sort_by(|&a, &b| {
        let (a, b) = (&prints[a as usize], &prints[b as usize]);
        a.key.cmp(&b.key).then(a.time_ms.total_cmp(&b.time_ms))
    });
    let mut actual = Vec::new();
    price_time_order(&prints, &mut actual);
    assert_eq!(actual, expected);
}

/// Rank selection must retain the old rounded percentile and invalid-volume rules at boundaries.
#[test]
fn selected_print_scale_matches_sorted_percentiles() {
    for len in [0, 1, 2, 3, 10, 63, 100, 101, 200] {
        let prints: Vec<_> = (0..len)
            .map(|i| {
                print(
                    i as f64,
                    1,
                    match i % 13 {
                        0 => f32::NAN,
                        1 => f32::INFINITY,
                        2 => -1.0,
                        3 => 0.0,
                        _ => ((i * 73) % 17 + 1) as f32,
                    },
                )
            })
            .collect();
        let mut sorted: Vec<_> = prints
            .iter()
            .map(Print::total)
            .filter(|v| v.is_finite() && *v > 0.0)
            .collect();
        sorted.sort_unstable_by(f32::total_cmp);
        for pct in [0, 25, 50, 75, 90, 99, 100, 255] {
            let expected = (!sorted.is_empty()).then(|| {
                let at =
                    |pct: f32| sorted[((pct / 100.0) * (sorted.len() - 1) as f32).round() as usize];
                let small = at(f32::from(pct.min(100)));
                PrintScale {
                    small,
                    big: at(99.0).max(small),
                }
            });
            assert_eq!(
                print_scale(&prints, pct, &mut Vec::new()),
                expected,
                "length={len}, pct={pct}"
            );
        }
    }
}

/// An occupied aggregation slot must keep the first timestamp and exact side-specific sum.
#[test]
fn reused_print_slots_preserve_expiry_and_out_of_order_trades() {
    let tape = [
        trade(10.0, 10.0, 2.0, true),
        trade(11.0, 11.0, 3.0, false),
        trade(15.0, 10.0, 4.0, true),
        trade(30.0, 10.0, 8.0, true),
        trade(29.0, 10.0, 16.0, true),
        trade(31.0, 10.0, 32.0, false),
        trade(32.0, 10.0, 64.0, true),
    ];
    let mut prints = Vec::new();
    aggregate_prints(tape, None, 10.0, &mut prints);
    let actual: Vec<_> = prints.iter().map(|p| (p.time_ms, p.buy, p.sell)).collect();
    assert_eq!(
        actual,
        [
            (10.0, 6.0, 0.0),
            (11.0, 0.0, 3.0),
            (30.0, 8.0, 0.0),
            (29.0, 80.0, 0.0),
            (31.0, 0.0, 32.0)
        ]
    );
}

/// Time all CPU bubble stages with 100000 distinct synthetic prints in a chart window.
#[test]
#[ignore = "manual heavy-user timing probe"]
fn bench_goal_b_clusters_revision() {
    use std::{hint::black_box, time::Instant};
    let tape: Vec<_> = (0..100_000)
        .map(|i| {
            (
                i as f64 * 10.0,
                10.0 + ((i * 73) % 1000) as f32 * 0.01,
                1.0 + (i % 37) as f32,
                i % 2 == 0,
            )
        })
        .collect();
    let (mut prints, mut order, mut scratch, mut discs, mut markers) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let start = Instant::now();
    for _ in 0..20 {
        aggregate_prints(
            black_box(&tape).iter().copied(),
            Some(0.01),
            100.0,
            &mut prints,
        );
        price_time_order(&prints, &mut order);
        let scale = print_scale(&prints, 50, &mut scratch).unwrap();
        merge_overlapping(
            &prints,
            &order,
            0.01,
            |v| print_radius(v, scale, DiscRadii::at(1.0)),
            &mut discs,
        );
        markers.clear();
        markers.extend(discs.iter().map(|d| {
            crate::layers::MarkerInstance::at_price(
                d.time_ms as f32,
                d.price,
                d.radius,
                0.0,
                crate::layers::MARKER_SHAPE_KNOT,
                [1.0; 4],
            )
        }));
        markers.sort_by(|a, b| a.size.total_cmp(&b.size));
        black_box(&markers);
    }
    println!(
        "goal_b_clusters_full: {} ns/iter",
        start.elapsed().as_nanos() / 20
    );
    for stage in 0..4 {
        let start = Instant::now();
        for _ in 0..20 {
            match stage {
                0 => aggregate_prints(
                    black_box(&tape).iter().copied(),
                    Some(0.01),
                    100.0,
                    &mut prints,
                ),
                1 => price_time_order(black_box(&prints), &mut order),
                2 => {
                    black_box(print_scale(black_box(&prints), 50, &mut scratch));
                }
                _ => {
                    // Production sorts freshly rebuilt markers, not the preceding frame's order.
                    markers.clear();
                    markers.extend(discs.iter().map(|d| {
                        crate::layers::MarkerInstance::at_price(
                            d.time_ms as f32,
                            d.price,
                            d.radius,
                            0.0,
                            crate::layers::MARKER_SHAPE_KNOT,
                            [1.0; 4],
                        )
                    }));
                    markers.sort_by(|a, b| a.size.total_cmp(&b.size));
                }
            }
        }
        println!(
            "goal_b_clusters_stage_{stage}: {} ns/iter",
            start.elapsed().as_nanos() / 20
        );
    }
}

/// Measure the Linux predicate over a million retained samples and three 100000-print windows.
/// This mirrors PlatformLayers::tick_samples; it does not instantiate the Linux-only GPU backend.
#[test]
#[ignore = "manual heavy-user tape-window timing probe"]
fn bench_goal_b_tape_window_collection() {
    use std::{hint::black_box, time::Instant};
    let tape: Vec<_> = (0..1_000_000)
        .map(|i| {
            (
                (i * 10) as f32,
                10.0 + ((i * 73) % 1000) as f32 * 0.01,
                1.0 + (i % 37) as f32,
                i % 2 == 0,
            )
        })
        .collect();
    // One visible window plus one window on either side; the remaining retained tape is excluded.
    let (from, to) = (1_000_000.0, 3_999_999.0);
    let start = Instant::now();
    for _ in 0..20 {
        let count = black_box(&tape)
            .iter()
            .filter(|c| f64::from(c.0) >= from && f64::from(c.0) <= to)
            .count();
        assert_eq!(black_box(count), 300_000);
    }
    println!(
        "goal_b_wgpu_predicate_mirror_1m_to_300k: {} ns/iter",
        start.elapsed().as_nanos() / 20
    );
    let start = Instant::now();
    for _ in 0..20 {
        let range = crate::tick_volume::tick_time_range(tape.len(), 0.0, from, to, |i| {
            black_box(&tape)[i].0
        });
        let count = black_box(&tape)[range]
            .iter()
            .filter(|c| f64::from(c.0) >= from && f64::from(c.0) <= to)
            .count();
        assert_eq!(black_box(count), 300_000);
    }
    println!(
        "goal_b2_wgpu_bounded_predicate_mirror_1m_to_300k: {} ns/iter",
        start.elapsed().as_nanos() / 20
    );
    let mut prints = Vec::new();
    let start = Instant::now();
    for _ in 0..5 {
        aggregate_prints(
            black_box(&tape)
                .iter()
                .filter(|c| f64::from(c.0) >= from && f64::from(c.0) <= to)
                .map(|c| (f64::from(c.0), c.1, c.2, c.3)),
            Some(0.01),
            100.0,
            &mut prints,
        );
        assert_eq!(prints.len(), 300_000);
        black_box(&prints);
    }
    println!(
        "goal_b_window_selection_and_fold_300k: {} ns/iter",
        start.elapsed().as_nanos() / 5
    );
}

/// A trade at `ms` of `notional` quote at `price`.
fn trade(ms: f64, price: f32, notional: f32, is_buy: bool) -> (f64, f32, f32, bool) {
    (ms, price, notional / price, is_buy)
}

fn print(ms: f64, key: i64, buy: f32) -> Print {
    Print {
        time_ms: ms,
        price: key as f32,
        key,
        buy,
        sell: 0.0,
    }
}

#[test]
fn trades_of_one_price_and_side_within_the_window_make_one_print() {
    let mut out = Vec::new();
    aggregate_prints(
        [
            trade(1_000.0, 10.0, 100.0, true),
            trade(1_060.0, 10.0, 50.0, true),
            // Past the window from the print's FIRST trade: a new print.
            trade(1_150.0, 10.0, 20.0, true),
            // Same time, other side: a new print.
            trade(1_150.0, 10.0, 30.0, false),
            // Other price: a new print.
            trade(1_160.0, 10.5, 40.0, false),
        ],
        Some(0.5),
        100.0,
        &mut out,
    );
    let shape: Vec<(f64, f32, f32)> = out.iter().map(|p| (p.time_ms, p.buy, p.sell)).collect();
    assert_eq!(shape.len(), 4);
    assert_eq!(shape[0].0, 1_000.0);
    assert!((shape[0].1 - 150.0).abs() < 1e-3);
    assert!((shape[1].1 - 20.0).abs() < 1e-3);
    assert!((shape[2].2 - 30.0).abs() < 1e-3);
    assert_eq!(out[3].key, 21);
}

/// A tape bouncing between the bid and the ask: the buys at one price still fold into one print.
#[test]
fn a_trade_at_another_price_does_not_end_a_print() {
    let mut out = Vec::new();
    aggregate_prints(
        [
            trade(0.0, 10.0, 1.0, true),
            trade(10.0, 9.5, 1.0, false),
            trade(20.0, 10.0, 2.0, true),
            trade(30.0, 9.5, 2.0, false),
        ],
        Some(0.5),
        100.0,
        &mut out,
    );
    assert_eq!(out.len(), 2);
    assert!((out[0].buy - 3.0).abs() < 1e-4);
    assert!((out[1].sell - 3.0).abs() < 1e-4);
}

#[test]
fn a_zero_window_still_joins_one_timestamp() {
    let mut out = Vec::new();
    aggregate_prints(
        [
            trade(5.0, 10.0, 1.0, true),
            trade(5.0, 10.0, 2.0, true),
            trade(6.0, 10.0, 4.0, true),
        ],
        None,
        0.0,
        &mut out,
    );
    assert_eq!(out.len(), 2);
    assert!((out[0].buy - 3.0).abs() < 1e-4);
}

#[test]
fn unusable_trades_make_no_print() {
    let mut out = Vec::new();
    aggregate_prints(
        [
            (f64::NAN, 10.0, 1.0, true),
            (1.0, 10.0, 0.0, true),
            (1.0, f32::NAN, 1.0, true),
            (1.0, -1.0, 1.0, true),
        ],
        Some(0.1),
        100.0,
        &mut out,
    );
    assert!(out.is_empty());
}

/// The scale is the market's own: every volume multiplied by a thousand — the same tape quoted in
/// another currency — draws the same discs.
#[test]
fn the_scale_does_not_depend_on_the_quote_currency() {
    let volumes: Vec<f32> = (1..=200).map(|i| (i * i) as f32).collect();
    let radii = |k: f32| {
        let prints: Vec<Print> = volumes
            .iter()
            .enumerate()
            .map(|(i, v)| print(i as f64, 1, v * k))
            .collect();
        let scale = print_scale(&prints, 50, &mut Vec::new()).expect("prints");
        prints
            .iter()
            .map(|p| print_radius(p.total(), scale, DiscRadii::at(1.0)).0)
            .collect::<Vec<_>>()
    };
    let (usdt, btc) = (radii(1.0), radii(1_000.0));
    for (a, b) in usdt.iter().zip(&btc) {
        assert!((a - b).abs() < 1e-3, "{a} vs {b}");
    }
}

#[test]
fn a_print_is_a_dot_below_the_small_one_and_full_from_the_big_one() {
    let scale = PrintScale {
        small: 100.0,
        big: 10_000.0,
    };
    let r = DiscRadii::at(1.0);
    assert_eq!(print_radius(99.0, scale, r), (DOT_RADIUS_PX, true));
    assert_eq!(print_radius(100.0, scale, r), (MIN_RADIUS_PX, false));
    assert_eq!(print_radius(1_000_000.0, scale, r), (MAX_RADIUS_PX, false));
    // Halfway in orders of magnitude: halfway in radius.
    let (mid, _) = print_radius(1_000.0, scale, r);
    assert!((mid - (MIN_RADIUS_PX + MAX_RADIUS_PX) * 0.5).abs() < 1e-3);
    // A coin whose prints are all one size: none stands out as a full disc.
    let flat = PrintScale {
        small: 50.0,
        big: 50.0,
    };
    assert_eq!(print_radius(50.0, flat, r), (MIN_RADIUS_PX, false));
    // ...but a print above that one size still does.
    assert_eq!(print_radius(500.0, flat, r), (MAX_RADIUS_PX, false));
}

/// A re-fold that barely moves the percentiles keeps the old scale, so the discs do not breathe.
#[test]
fn a_scale_holds_against_a_small_drift() {
    let old = PrintScale {
        small: 100.0,
        big: 10_000.0,
    };
    let near = PrintScale {
        small: 105.0,
        big: 9_500.0,
    };
    let far = PrintScale {
        small: 100.0,
        big: 20_000.0,
    };
    assert!(old.holds_against(&near));
    assert!(!old.holds_against(&far));
}

#[test]
fn covering_discs_of_one_price_merge_and_grow() {
    let prints = [
        print(0.0, 1, 100.0),
        print(1_000.0, 1, 100.0),
        print(2_000.0, 1, 100.0),
        // Another price at the same time never merges with the first row.
        print(0.0, 2, 100.0),
    ];
    let mut order = Vec::new();
    price_time_order(&prints, &mut order);
    // Radius grows with the volume, so a merged disc is bigger than one print's.
    let size = |v: f32| (v / 10.0, false);
    let mut out = Vec::new();
    // 1 px per 100 ms: the three prints of row 1 sit 10 px apart with 10 px discs — they cover,
    // and the grown disc keeps swallowing the next.
    merge_overlapping(&prints, &order, 0.01, size, &mut out);
    assert_eq!(out.len(), 2);
    assert!((out[0].total() - 300.0).abs() < 1e-3);
    assert_eq!(out[0].radius, 30.0);
    assert_eq!(out[0].time_ms, 1_000.0);
    assert_eq!(out[0].first_ms, 0.0);
    // Zoomed in to 1 px per ms they stand apart.
    merge_overlapping(&prints, &order, 1.0, size, &mut out);
    assert_eq!(out.len(), 4);
}

#[test]
fn stored_settings_snap_onto_the_lists() {
    assert_eq!(snap_merge_ms(120), 100);
    assert_eq!(snap_merge_ms(9_999), 1_000);
    assert_eq!(snap_small_pct(60), 50);
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
    // Below one: no "0.0" that reads as an empty disc, no "1.0" beside a plain "1".
    assert_eq!(short_volume(0.04), "<0.1");
    assert_eq!(short_volume(0.97), "1");
    assert_eq!(short_volume(999.7), "1K");
    assert_eq!(short_volume(f32::NAN), "");
}
