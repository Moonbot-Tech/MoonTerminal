//! Synthetic detector contracts; no live data or exchange state participates.

use super::probe::{Probe, ticker_tape};
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

/// A live wall slot replaces earlier prints and cannot borrow a later print.
#[test]
fn wall_slots_last_wins_without_lookahead() {
    let ticks = tape(&[
        (0, 100.0),
        (2100, 90.0),
        (2900, 80.0),
        (3100, 70.0),
        (4100, 60.0),
    ]);
    let v = DropsVariant {
        wall_slots: true,
        ..Default::default()
    };
    assert_eq!(v.samples(&ticks, 0, 3000), vec![(0, 100.0), (2000, 80.0)]);
    assert_eq!(v.samples(&ticks, 0, 3200), vec![(0, 100.0), (2000, 70.0)]);
}

/// Including the live slot changes the average immediately, independently of the grid.
#[test]
fn current_average_includes_live_slot() {
    let ticks = tape(&[(-10000, 100.0), (8000, 80.0), (10000, 60.0)]);
    let p = DropsParams {
        last_price_ma: 2,
        ..params()
    };
    for wall_slots in [false, true] {
        let v = DropsVariant {
            wall_slots,
            include_current: true,
            ..Default::default()
        };
        assert_eq!(
            p.drop_at_variant(&ticks, 10000, v).unwrap().last_price,
            70.0
        );
        assert_eq!(
            p.drop_at_variant(
                &ticks,
                10000,
                DropsVariant {
                    include_current: false,
                    ..v
                }
            )
            .unwrap()
            .last_price,
            90.0
        );
    }
}

/// Consecutive-sample smoothing reproduces the independent reference FAQ arithmetic.
#[test]
fn rolling_high_matches_reference_faq() {
    let points: Vec<_> = (-20..50)
        .map(|i| {
            (
                i * 2000,
                if i < 10 {
                    102.0
                } else if i < 40 {
                    101.0
                } else {
                    100.0
                },
            )
        })
        .collect();
    let p = DropsParams {
        max_time_s: 100,
        price_ma_s: 20,
        last_price_ma: 2,
        ..params()
    };
    let r = p
        .drop_at_variant(&tape(&points), 98000, DropsVariant::ASTER)
        .unwrap();
    assert_eq!((r.high_zone_avg, r.last_price), (102.0, 100.0));
    assert!((r.drop_pct - 2.0).abs() < 1e-9);
}

/// A threshold between passes distinguishes second, slot-boundary and print schedules.
#[test]
fn decision_cadences_are_independent() {
    let ticks = tape(&[(-10000, 100.0), (7100, 90.0)]);
    for (cadence, expected) in [
        (DropsCadence::Second, 7500),
        (DropsCadence::Slot, 8000),
        (DropsCadence::Print, 7100),
    ] {
        let v = DropsVariant {
            cadence,
            ..Default::default()
        };
        assert_eq!(
            params().detect_variant(&ticks, 6500, 10000, v).unwrap().0,
            expected
        );
    }
}

/// Delaying placement must not claim the print that crossed before the order existed.
#[test]
fn order_delay_skips_preplacement_fill() {
    let mut d = deal();
    d.buy_ms = 70000;
    d.buy_set_ms = Some(70000);
    let ticks = tape(&[
        (0, 100.0),
        (70000, 90.0),
        (71000, 80.0),
        (72000, 90.0),
        (73000, 79.0),
    ]);
    let p = DropsParams {
        buy_price_pct: -1.0,
        ..params()
    };
    let entry = DropsEntry(&p);
    let old = entry.trace(&d, &ticks);
    let delayed = entry.trace_variant(
        &d,
        &ticks,
        DropsVariant {
            delay_slot: true,
            ..Default::default()
        },
    );
    assert_eq!(old.detect_ms, delayed.detect_ms);
    assert_eq!(old.order_ms, Some(70000));
    assert_eq!(delayed.order_ms, Some(72000));
    assert_eq!(old.fill.unwrap().t_ms, 71000);
    assert_eq!(delayed.fill.unwrap().t_ms, 73000);
}

/// Explicit default replay must preserve the old independently known detector and limit values.
#[test]
fn default_variant_preserves_legacy_outputs() {
    let ticks = tape(&[(-10000, 100.0), (8000, 80.0), (10000, 60.0)]);
    let p = DropsParams {
        last_price_ma: 2,
        use_last_price: true,
        buy_price_pct: -1.0,
        ..params()
    };
    let r = p
        .drop_at_variant(&ticks, 10000, DropsVariant::default())
        .unwrap();
    assert_eq!(r, p.drop_at(&ticks, 10000).unwrap());
    assert_eq!((r.high_zone_avg, r.last_price), (100.0, 90.0));
    assert!((p.buy_price(r, &ticks, 10000, 0.1).unwrap() - 89.1).abs() < 1e-9);
}

/// Lag changes placement and fill visibility while preserving the original signal time.
#[test]
fn probe_fixed_lags_place_after_signal() {
    let mut d = deal();
    d.buy_ms = 70000;
    d.buy_set_ms = Some(70000);
    let ticks = tape(&[(0, 100.0), (70000, 90.0), (80000, 89.0)]);
    for lag_s in [1, 2, 3, 5, 10] {
        let trace = Probe {
            lag_s,
            ..Default::default()
        }
        .trace(&params(), &d, &ticks, &ticks, None);
        assert_eq!(trace.detect_ms, Some(70000));
        assert_eq!(trace.order_ms, Some(70000 + i64::from(lag_s) * 1000));
        assert_eq!(trace.fill.unwrap().t_ms, 80000);
    }
}

/// A rebound between two drops resets the pass run, rather than accumulating old successes.
#[test]
fn probe_hold_requires_consecutive_seconds() {
    let ticks = tape(&[(-30000, 100.0), (7000, 90.0), (8000, 100.0), (9000, 90.0)]);
    let p = DropsParams {
        max_time_s: 20,
        ..params()
    };
    for hold in [2, 3, 5] {
        assert_eq!(
            Probe {
                hold,
                ..Default::default()
            }
            .detect(&p, &ticks, 7000, 15000, None)
            .unwrap()
            .0,
            9000 + i64::from(hold - 1) * 1000
        );
    }
}

/// A recovered current price must still expose the earlier minimum only in this probe.
#[test]
fn probe_high_to_min_retains_recovered_low() {
    let ticks = tape(&[(0, 100.0), (8000, 80.0), (10000, 100.0)]);
    assert_eq!(
        Probe::default()
            .reading(&params(), &ticks, 10000)
            .unwrap()
            .drop_pct,
        0.0
    );
    assert_eq!(
        Probe {
            high_to_min: true,
            ..Default::default()
        }
        .reading(&params(), &ticks, 10000)
        .unwrap()
        .drop_pct,
        25.0
    );
}

/// Future slot prints stay invisible until close plus one second, and both sides form a mid.
#[test]
fn probe_ticker_waits_for_slot_and_uses_side_mid() {
    let mut ticks = tape(&[(100, 100.0), (900, 110.0), (1500, 90.0), (2100, 80.0)]);
    ticks[2].side = Side::Sell;
    let input = ticker_tape(&ticks);
    assert_eq!(last_price(&input, 2999, None), None);
    assert_eq!(last_price(&input, 3000, None), Some(100.0));
    assert_eq!(last_price(&input, 4999, None), Some(100.0));
    assert_eq!(last_price(&input, 5000, None), Some(80.0));
}

/// The strategy switch gates both horizons and missing hourly history remains unknown.
#[test]
fn probe_low_horizons_require_coverage_and_new_low() {
    let ticks = tape(&[
        (0, 80.0),
        (3540000, 90.0),
        (3599000, 100.0),
        (3600000, 90.0),
        (3601000, 70.0),
    ]);
    let p = DropsParams {
        price_is_low: true,
        ..params()
    };
    assert_eq!(
        Probe {
            low_s: 60,
            ..Default::default()
        }
        .low_pass(&p, &ticks, 3600000),
        Some(true)
    );
    assert_eq!(
        Probe {
            low_s: 3600,
            ..Default::default()
        }
        .low_pass(&p, &ticks, 3600000),
        Some(false)
    );
    assert_eq!(
        Probe {
            low_s: 3600,
            ..Default::default()
        }
        .low_pass(&p, &ticks, 3601000),
        Some(true)
    );
    assert_eq!(
        Probe {
            low_s: 3600,
            ..Default::default()
        }
        .low_pass(&p, &ticks[1..], 3600000),
        None
    );
    assert_eq!(
        Probe {
            low_s: 3600,
            ..Default::default()
        }
        .low_pass(&params(), &ticks[1..], 3600000),
        Some(true)
    );
}

/// A previous factual detect blocks checks strictly before the expiry and not at equality.
#[test]
fn probe_previous_deal_penalty_expires_at_boundary() {
    let ticks = tape(&[(-30000, 100.0), (7000, 90.0)]);
    let p = DropsParams {
        max_time_s: 20,
        ..params()
    };
    let probe = Probe {
        suppress: true,
        ..Default::default()
    };
    assert!(
        probe
            .detect(&p, &ticks, 7000, 9000, Some((5000, 10000)))
            .is_none()
    );
    assert_eq!(
        probe
            .detect(&p, &ticks, 7000, 11000, Some((5000, 10000)))
            .unwrap()
            .0,
        10000
    );
    assert_eq!(
        probe
            .detect(&p, &ticks, 7000, 9000, Some((8000, 10000)))
            .unwrap()
            .0,
        7000
    );
}

/// Combining a hold and a lag must apply both clocks rather than replacing either one.
#[test]
fn probe_combined_hold_and_lag_keep_distinct_clocks() {
    let mut d = deal();
    d.buy_ms = 70000;
    d.buy_set_ms = Some(70000);
    let ticks = tape(&[(0, 100.0), (70000, 90.0), (80000, 89.0)]);
    let trace = Probe {
        hold: 3,
        lag_s: 2,
        ..Default::default()
    }
    .trace(&params(), &d, &ticks, &ticks, None);
    assert_eq!(trace.detect_ms, Some(72000));
    assert_eq!(trace.order_ms, Some(74000));
    assert_eq!(trace.fill.unwrap().t_ms, 80000);
}

/// Doubling the window and flooring it retain an older high excluded by seconds.
#[test]
fn unit_window_scaling_and_floor_retain_old_high() {
    let ticks = tape(&[(0, 100.0), (24000, 140.0), (28000, 90.0)]);
    let p = params();
    let u = units::Units::default();
    assert_eq!(u.reading(&p, &ticks, 32000).unwrap().high_zone_avg, 90.0);
    for window in [1, 2] {
        assert_eq!(
            units::Units { window, ..u }
                .reading(&p, &ticks, 32000)
                .unwrap()
                .high_zone_avg,
            140.0
        );
    }
}

/// Seconds, sample counts and one-second zones give different means on a transient peak.
#[test]
fn unit_high_ma_interpretations_average_distinct_endpoints() {
    let ticks = tape(&[(0, 100.0), (25000, 140.0), (27000, 80.0), (29000, 60.0)]);
    let p = DropsParams {
        max_time_s: 8,
        price_ma_s: 2,
        ..params()
    };
    let u = units::Units::default();
    assert_eq!(u.reading(&p, &ticks, 30000).unwrap().high_zone_avg, 140.0);
    assert_eq!(
        units::Units { high: 1, ..u }
            .reading(&p, &ticks, 30000)
            .unwrap()
            .high_zone_avg,
        120.0
    );
    assert_eq!(
        units::Units { high: 2, ..u }
            .reading(&p, &ticks, 30000)
            .unwrap()
            .high_zone_avg,
        140.0
    );
    let p = DropsParams { price_ma_s: 3, ..p };
    assert!(
        (units::Units { high: 2, ..u }
            .reading(&p, &ticks, 30000)
            .unwrap()
            .high_zone_avg
            - 380.0 / 3.0)
            .abs()
            < 1e-9
    );
}

/// A last MA expressed in seconds uses one two-second endpoint instead of two.
#[test]
fn unit_last_ma_seconds_preserve_recent_low() {
    let ticks = tape(&[(0, 100.0), (30000, 60.0)]);
    let p = DropsParams {
        last_price_ma: 2,
        ..params()
    };
    let u = units::Units::default();
    assert_eq!(u.reading(&p, &ticks, 30000).unwrap().last_price, 80.0);
    assert_eq!(
        units::Units {
            last_seconds: true,
            ..u
        }
        .reading(&p, &ticks, 30000)
        .unwrap()
        .last_price,
        60.0
    );
}

/// Extending zone starts admits an older high but still refuses unavailable warm-up.
#[test]
fn unit_extended_zones_need_pre_window_history() {
    let ticks = tape(&[(0, 100.0), (24000, 160.0), (28000, 80.0)]);
    let p = DropsParams {
        price_ma_s: 4,
        ..params()
    };
    let u = units::Units::default();
    assert_eq!(u.reading(&p, &ticks, 32000).unwrap().high_zone_avg, 80.0);
    assert_eq!(
        units::Units { extend: true, ..u }
            .reading(&p, &ticks, 32000)
            .unwrap()
            .high_zone_avg,
        120.0
    );
    assert!(
        units::Units { extend: true, ..u }
            .reading(&p, &ticks[1..], 28000)
            .is_none()
    );
}

/// Odd window lengths must retain decision-aligned samples instead of shifting their phase.
#[test]
fn unit_odd_window_keeps_decision_sample_phase() {
    let ticks = tape(&[(0, 100.0), (25000, 150.0), (26000, 80.0), (29000, 60.0)]);
    let p = DropsParams {
        max_time_s: 7,
        ..params()
    };
    assert_eq!(
        units::Units::default()
            .reading(&p, &ticks, 30000)
            .unwrap()
            .high_zone_avg,
        100.0
    );
}
