//! Synthetic detector contracts; no live data or exchange state participates.

use super::*;
use crate::db::tuner::ticks::{EntryModel, tests::deal};

/// Buy-side prints let the independent expected levels use the same trade and ASK reference.
fn tape(points: &[(i64, f64)]) -> Vec<Tick> {
    points
        .iter()
        .map(|&(t, p)| Tick {
            time_ms: t as f64,
            price: p as f32,
            qty: 1.0,
            side: Side::Buy,
        })
        .collect()
}

/// A short analysis period keeps every arithmetic oracle visible in each test.
fn params() -> DropsParams {
    DropsParams {
        max_time_s: 4,
        price_delta_pct: 5.0,
        buy_price_last_trade: true,
        ..Default::default()
    }
}

/// Carrying the last price across a hole must not drop samples or invent zero prices.
#[test]
fn samples_carry_forward_empty_slots() {
    assert_eq!(
        samples(&tape(&[(0, 100.0), (5_000, 90.0)]), 1_000, 7_000),
        vec![(1_000, 100.0), (3_000, 100.0), (5_000, 90.0), (7_000, 90.0)]
    );
}

/// Sliding zones must average their prices before taking the highest zone, not the highest print.
#[test]
fn zone_average_matches_faq_example() {
    let p = DropsParams {
        max_time_s: 6,
        price_ma_s: 4,
        ..params()
    };
    // At t=10s the three four-second zones have means 110, 110, 90.
    let ticks = tape(&[(0, 100.0), (5_000, 120.0), (7_000, 100.0), (9_000, 80.0)]);
    let reading = p.drop_at(&ticks, 10_000).unwrap();
    assert_eq!(reading.high_zone_avg, 110.0);
    assert_eq!(reading.last_price, 80.0);
    assert!((reading.drop_pct - 37.5).abs() < 1e-9);
}

/// A sub-two-second MA must not erase a high exchange sample with an extra averaging window.
#[test]
fn price_ma_under_two_uses_raw_samples() {
    let ticks = tape(&[(0, 100.0), (4_000, 120.0), (5_000, 80.0)]);
    for width in [0, 1] {
        let p = DropsParams {
            price_ma_s: width,
            ..params()
        };
        assert_eq!(p.drop_at(&ticks, 6_000).unwrap().high_zone_avg, 120.0);
    }
}

/// Using the current print for MA=1 would incorrectly let a transient new low trigger it.
#[test]
fn last_price_ma_zero_fires_on_spike_and_one_does_not() {
    let ticks = tape(&[(-2_000, 100.0), (6_000, 90.0)]);
    assert!(params().detect(&ticks, 6_000, 6_000).is_some());
    let p = DropsParams {
        last_price_ma: 1,
        ..params()
    };
    assert!(p.detect(&ticks, 8_000, 8_000).is_some());
    assert!(p.detect(&ticks, 6_000, 6_000).is_none());
}

/// Threshold checks must return the first decision second, without looking at later prints.
#[test]
fn detect_fires_first_second_crossing_delta() {
    let ticks = tape(&[(0, 100.0), (7_100, 90.0), (9_000, 80.0)]);
    assert_eq!(params().detect(&ticks, 6_000, 10_000).unwrap().0, 8_000);
}

/// Short warm-up must refuse instead of treating the first available print as complete history.
#[test]
fn refuses_when_tape_shorter_than_needed_history() {
    let p = DropsParams {
        last_price_ma: 2,
        ..params()
    };
    assert_eq!(p.needed_history_ms(), 10_000);
    assert!(
        p.drop_at(&tape(&[(1, 100.0), (10_000, 90.0)]), 10_000)
            .is_none()
    );
    assert!(
        p.drop_at(&tape(&[(0, 100.0), (10_000, 90.0)]), 10_000)
            .is_some()
    );
    let mut d = deal();
    d.kind = KIND_DROPS.into();
    assert!(DropsEntry(&p).trace(&d, &[]).refused);
    // Even a later covered crossing cannot establish that the uncovered earlier seconds did not fire.
    d.buy_ms = 70_000;
    let partial = DropsEntry(&p).trace(&d, &tape(&[(1, 100.0), (70_000, 90.0)]));
    assert!(partial.refused && partial.fill.is_none() && partial.detect_ms.is_none());
}

/// Omitting the buy-reference warm-up would replay a minimum over incomplete history.
#[test]
fn refuses_when_30s_reference_lacks_history() {
    let mut p = DropsParams {
        use_30s_old_ask: true,
        ..params()
    };
    let mut d = deal();
    d.buy_ms = 70_000;
    d.buy_set_ms = Some(70_000);
    let ticks = tape(&[(-19_999, 100.0), (70_000, 90.0)]);
    assert_eq!(p.needed_history_ms(), 30_000);
    assert!(DropsEntry(&p).trace(&d, &ticks).refused);
    assert!(p.drop_at(&ticks, 10_000).is_none());
    assert!(
        !DropsEntry(&p)
            .trace(&d, &tape(&[(-20_000, 100.0), (70_000, 90.0)]))
            .refused
    );
    p.use_last_price = true;
    assert_eq!(p.needed_history_ms(), 6_000);
    assert!(!DropsEntry(&p).trace(&d, &ticks).refused);
    p.use_last_price = false;
    p.max_time_s = 40;
    assert_eq!(p.needed_history_ms(), 42_000);
}

/// The detector reference must win over the current print when DropsUseLastPrice is on.
#[test]
fn use_last_price_buys_off_averaged_last() {
    let p = DropsParams {
        last_price_ma: 2,
        use_last_price: true,
        buy_price_pct: -1.0,
        ..params()
    };
    let ticks = tape(&[(0, 100.0), (8_000, 80.0), (10_000, 60.0)]);
    let r = p.drop_at(&ticks, 10_000).unwrap();
    assert_eq!(r.last_price, 90.0);
    assert!((p.buy_price(r, &ticks, 10_000, 0.1).unwrap() - 89.1).abs() < 1e-9);
}

/// A limit above the current price must execute at the crossing print rather than overpay.
#[test]
fn buy_price_pct_positive_fills_immediately() {
    let p = DropsParams {
        buy_price_pct: 1.0,
        ..params()
    };
    let mut d = deal();
    d.buy_ms = 70_000;
    d.buy_set_ms = Some(70_000);
    let ticks = tape(&[(0, 100.0), (70_000, 90.0)]);
    let fill = DropsEntry(&p).fill(&d, &ticks, None).unwrap();
    assert_eq!(fill.t_ms, 70_000);
    assert!((fill.price - 90.0).abs() < 1e-9);
}

/// Marketable orders receive the print price while resting orders retain their limit.
#[test]
fn marketable_limit_fills_at_print() {
    let mut d = deal();
    d.buy_ms = 70_000;
    d.buy_set_ms = Some(70_000);
    let ticks = tape(&[(0, 100.0), (70_000, 90.0), (71_000, 88.0)]);
    for (offset, expected_time, expected_price) in [(1.0, 70_000, 90.0), (-1.0, 71_000, 89.1)] {
        let p = DropsParams {
            buy_price_pct: offset,
            ..params()
        };
        let fill = DropsEntry(&p).trace(&d, &ticks).fill.unwrap();
        assert_eq!(fill.t_ms, expected_time);
        assert!((fill.price - expected_price).abs() < 1e-9);
    }
}

/// Impossible zone widths must not inflate the missing-history refusal count.
#[test]
fn degenerate_params_not_reported_as_refused() {
    let p = DropsParams {
        price_ma_s: 5,
        ..params()
    };
    let mut d = deal();
    d.buy_ms = 70_000;
    d.buy_set_ms = Some(70_000);
    let trace = DropsEntry(&p).trace(&d, &tape(&[(0, 100.0), (70_000, 90.0)]));
    assert!(trace.degenerate);
    assert!(!trace.refused);
    assert!(trace.detect_ms.is_none() && trace.fill.is_none());
}

/// Core spelling variants and a percent suffix must retain factual detector evidence.
#[test]
fn parse_comment_tolerates_case_and_percent() {
    for (value, expected) in [
        ("True", true),
        ("FALSE", false),
        ("YeS", true),
        ("No", false),
        ("1", true),
        ("0", false),
    ] {
        let parsed = parse_comment(&format!(
            "DropsDetection: PriceIsLow: {value} xPriceDelta: 3.25%"
        ))
        .unwrap();
        assert_eq!(parsed.price_is_low, expected);
        assert!((parsed.x_price_delta_pct - 3.25).abs() < 1e-9);
    }
}

/// Reusing the long formula on shorts would fabricate unsupported entries.
#[test]
fn short_side_not_modelled() {
    let mut d = deal();
    d.buy_ms = 70_000;
    let ticks = tape(&[(0, 100.0), (70_000, 90.0)]);
    assert!(DropsEntry(&params()).fill(&d, &ticks, None).is_some());
    d.is_short = true;
    assert!(DropsEntry(&params()).fill(&d, &ticks, None).is_none());
}

/// Calibration needs detect, reading and limit even when the order has not filled yet.
#[test]
fn trace_reports_detect_and_limit_on_synthetic_tape() {
    let p = DropsParams {
        buy_price_pct: -1.0,
        ..params()
    };
    let mut d = deal();
    d.buy_ms = 70_000;
    d.buy_set_ms = Some(70_000);
    d.coin = "XALPHA".into();
    d.core_name = "CORE_A".into();
    let ticks = tape(&[(0, 100.0), (70_000, 90.0), (81_000, 89.0)]);
    let trace = DropsEntry(&p).trace(&d, &ticks);
    assert!(!trace.refused);
    assert_eq!(trace.detect_ms, Some(70_000));
    assert!((trace.reading.unwrap().drop_pct - 100.0 / 9.0).abs() < 1e-9);
    assert_eq!(trace.limit, Some(89.1));
    assert_eq!(
        trace.fill,
        Some(Fill {
            t_ms: 81_000,
            price: 89.1
        })
    );
    let no_fill = DropsEntry(&p).trace(&d, &ticks[..2]);
    assert!(no_fill.limit.is_some() && no_fill.fill.is_none() && !no_fill.refused);
}

/// Read the two factual fields from the drop header without requiring a percent suffix.
#[test]
fn parse_comment_reads_xpricedelta_and_pricelow() {
    assert_eq!(
        parse_comment(
            " DropsDetection: CORE_A USDT-XALPHA DailyVol: 27m PriceIsLow: false xPriceDelta: 3.0 \r\nBTC(5min, 1sec) = 0"
        ),
        Some(DropsDetect {
            x_price_delta_pct: 3.0,
            price_is_low: false
        })
    );
}

/// Other detectors and malformed numeric fields must not lend DropsDetection factual evidence.
#[test]
fn parse_comment_rejects_other_kinds() {
    assert!(parse_comment("PumpsDetection: PriceIsLow: false xPriceDelta: 3.0").is_none());
    assert!(parse_comment("DropsDetection: PriceIsLow: false xPriceDelta: NaN").is_none());
}
