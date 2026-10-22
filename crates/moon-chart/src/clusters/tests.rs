//! Unit tests for the trade prints: folding, the market's own scale and merging on zoom.

use super::*;

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
