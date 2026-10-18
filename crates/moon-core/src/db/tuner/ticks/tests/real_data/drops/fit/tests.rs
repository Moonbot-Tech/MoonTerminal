//! Independent arithmetic and timing oracles for the fixed-cohort fit engine.

use super::*;
use crate::db::tuner::ticks::tests::tick;
use crate::feed::types::Side;

/// Keep the original linear scan independent of the hoisted crossing masks.
fn naive_first(
    readings: &mut Readings,
    point: FitPoint,
    p: &DropsParams,
    ticks: &[Tick],
    from: i64,
    to: i64,
) -> Option<i64> {
    (from..=to).step_by(1000).find(|&s| {
        readings
            .at(point, p, ticks, s)
            .is_some_and(|r| r.drop_pct >= p.price_delta_pct)
    })
}

/// Deterministic irregular prints exercise slots, side filtering, and missing warm-up.
fn random_tape(mut seed: u64) -> Vec<Tick> {
    (0..550)
        .map(|i| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            tick(
                i * 1000 + (seed % 900) as i64,
                98.0 + ((seed >> 32) % 700) as f64 / 100.0,
                if seed & 16 == 0 {
                    Side::Sell
                } else {
                    Side::Buy
                },
            )
        })
        .collect()
}

/// Window hoisting must preserve the first crossing, including differently phased log resets.
#[test]
fn hoisted_first_fire_matches_naive_scan() {
    for seed in [1, 17, 903] {
        let ticks = random_tape(seed);
        let mut fast = Readings::default();
        let mut naive = Readings::default();
        for p in FitPoint::grid().filter(|p| {
            [2, 7, 20, 299, 300].contains(&p.w_s)
                && [1, 3, 30].contains(&p.k)
                && [0, 2, 6].contains(&p.j)
        }) {
            for (from, to) in [(440000, 502000), (497123, 502000), (140000, 202000)] {
                assert_eq!(
                    fast.first(p, &params(), &ticks, from, to),
                    naive_first(&mut naive, p, &params(), &ticks, from, to),
                    "seed={seed} point={p:?} from={from}"
                );
            }
        }
    }
}

/// Restoring a one-second wall candidate would admit an uncomputable production hypothesis.
#[test]
fn grid_has_no_wall_slot_step_1000_points() {
    assert!(FitPoint::grid().all(|p| !(p.wall_slots && p.step_ms == 1000)));
}

/// Cached means and side-specific current prices must match direct readers in both slot modes.
#[test]
fn cached_decision_matches_direct_reader_all_variants() {
    let ticks = random_tape(71);
    let t = 500123;
    let wall: Vec<_> = wall_samples(&ticks, ticks[0].time_ms as i64, t, usize::MAX)
        .into_iter()
        .map(|(at, price)| tick(at, price, Side::Buy))
        .collect();
    let mut cache = Readings::default();
    for p in FitPoint::grid().filter(|p| {
        [2, 3, 20, 299, 300].contains(&p.w_s)
            && [1, 3, 30].contains(&p.k)
            && [0, 2, 6].contains(&p.j)
    }) {
        let units = Units {
            explicit: Some((p.w_s as u32, p.k, p.j, p.step_ms)),
            extend: p.extend,
            current: p.current,
            ..Default::default()
        };
        let input = if p.wall_slots { &wall[..] } else { &ticks[..] };
        let expected = units.reading(&params(), input, t).map(|mut r| {
            if p.wall_slots && (matches!(p.current, Current::Print | Current::AskProxy) || p.j == 0)
            {
                r.last_price = units.reading(&params(), &ticks, t).unwrap().last_price;
                r.drop_pct = (r.high_zone_avg / r.last_price - 1.0) * 100.0;
            }
            r
        });
        match (expected, cache.at(p, &params(), &ticks, t)) {
            (Some(a), Some(b)) => {
                assert!((a.drop_pct - b.drop_pct).abs() < 1e-10, "{p:?}");
                assert!((a.high_zone_avg - b.high_zone_avg).abs() < 1e-10, "{p:?}");
                assert_eq!(a.last_price, b.last_price, "{p:?}");
            }
            (None, None) => {}
            _ => panic!("cached/direct availability mismatch: {p:?}"),
        }
    }
}

/// Quote-first, market-suffix, and task-prefix spellings must share the diagnostic reset.
#[test]
fn logged_reset_matches_coin_key_spellings() {
    let row = detect(500000, 5.0, 100.0);
    let ticks = vec![
        tick(0, 105.0, Side::Buy),
        tick(440000, 100.0, Side::Buy),
        tick(502000, 100.0, Side::Buy),
    ];
    for coin in ["XALPHA", "USDT-XALPHA", "XALPHA<Short>", "XALPHA:"] {
        let mut previous = detect(499000, 5.0, 100.0);
        previous.coin = coin.into();
        let fit = fit_detect(
            &row,
            &params(),
            &ticks,
            &envelope(500000),
            Some(&previous),
            &[FitPoint {
                w_s: 300,
                k: 1,
                j: 0,
                ..point()
            }],
        );
        assert!(fit.errors[0].logged_fire2s, "{coin}");
        assert!(!fit.errors[0].fire2s);
    }
}

/// Invalid source thresholds and unsupported current means must be excluded before scoring.
#[test]
fn nonpositive_delta_and_large_last_ma_are_degenerate() {
    for p in [
        DropsParams {
            price_delta_pct: 0.0,
            ..params()
        },
        DropsParams {
            price_delta_pct: -1.0,
            ..params()
        },
        DropsParams {
            last_price_ma: 7,
            ..params()
        },
    ] {
        let fit = fit_detect(
            &detect(500000, 4.8, 98.0),
            &p,
            &tape(),
            &envelope(500000),
            None,
            &[point()],
        );
        assert_eq!(fit.refusal, Some(Refusal::DegenerateParams));
        assert!(fit.errors[0].drop_error.is_infinite());
        assert_eq!(fit.passing, [0]);
    }
}

/// Measure the full candidate grid on 1000 synthetic detects without accessing real data.
#[test]
#[ignore = "synthetic full-grid timing probe"]
fn fit_speed_probe_synthetic_1000_detects() {
    let points: Vec<_> = FitPoint::grid().collect();
    let started = std::time::Instant::now();
    let mut hits = 0;
    let mut rows = Vec::with_capacity(1000);
    for i in 0..1000 {
        let ticks = random_tape(i + 1);
        let fitted = fit_detect(
            &detect(500123, 4.8, 100.0),
            &params(),
            &ticks,
            &envelope(500123),
            None,
            &points,
        );
        hits += fitted
            .passing
            .iter()
            .map(|v| v.count_ones() as usize)
            .sum::<usize>();
        rows.push((params(), fitted.passing));
        if (i + 1) % 100 == 0 {
            eprintln!(
                "FIT SPEED progress={} elapsed_seconds={:.3}",
                i + 1,
                started.elapsed().as_secs_f64()
            );
        }
    }
    let fit_seconds = started.elapsed().as_secs_f64();
    let search = std::time::Instant::now();
    let winners = super::super::heldout::synthetic_rule_search(rows, &points);
    eprintln!(
        "FIT SPEED synthetic_detects=1000 points={} elapsed_seconds={:.3} fit_seconds={fit_seconds:.3} rule_search_seconds={:.3} hits={hits} forward_family={} reverse_family={}",
        points.len(),
        started.elapsed().as_secs_f64(),
        search.elapsed().as_secs_f64(),
        winners[0],
        winners[1]
    );
}

/// A neutral logged row fixes a one-second decision phase without reading any real logs.
fn detect(t: i64, xpd: f64, ask: f64) -> LogDetect {
    let text = format!(
        "00:08:20.000 INFO DropsDetection: <STRAT_A> XALPHA DailyVol: 1m PriceIsLow: false xPriceDelta: {xpd}\n"
    );
    let mut row = super::super::logs::parse_core_log("CORE_A", "1970-01-01", &text)
        .records
        .remove(0);
    row.detect_ms = t;
    row.ask = Some(ask);
    row
}

/// All candidates share these source parameters; window interpretation is a separate hypothesis.
fn params() -> DropsParams {
    DropsParams {
        max_time_s: 20,
        price_ma_s: 3,
        last_price_ma: 2,
        price_delta_pct: 1.0,
        ..Default::default()
    }
}

/// The planted point averages the last two endpoints and complete three-sample zones.
fn point() -> FitPoint {
    FitPoint {
        w_s: 20,
        k: 3,
        j: 2,
        step_ms: 2000,
        extend: false,
        current: Current::MeanIncludingSlot,
        wall_slots: false,
    }
}

/// Old highs distinguish a long window; the planted recent high starts crossing only at t-2s.
fn tape() -> Vec<Tick> {
    [
        (0, 100.0),
        (410000, 110.0),
        (414000, 100.0),
        (494000, 104.0),
        (498000, 98.0),
        (500000, 98.0),
        (502000, 98.0),
    ]
    .into_iter()
    .map(|(t, p)| tick(t, p, Side::Buy))
    .collect()
}

/// The hand-computed recent zone is (104+104+100)/3 and the current mean is 98.
#[test]
fn fit_finds_planted_config_on_synthetic_tape() {
    let row = detect(500000, 4.8, 98.0);
    let points = [point(), FitPoint { w_s: 60, ..point() }];
    let fitted = fit_detect(
        &row,
        &params(),
        &tape(),
        &envelope(row.detect_ms),
        None,
        &points,
    );
    assert_eq!(fitted.refusal, None);
    assert!(fitted.passes(0));
    assert!(!fitted.passes(1));
    assert!(!fitted.errors[1].fire2s);
    assert!(fitted.errors[0].ask01);
    assert_eq!(fitted.passing, [1]);
    assert!(!fitted.passes(64));
    // Independent direct-zone evaluation also covers odd windows and the extended grid edges.
    let ticks = tape();
    let mut readings = Readings::default();
    for w in [2, 3, 20, 299, 300] {
        for k in [1, 3, 30] {
            for j in [0, 2, 6] {
                for step_ms in [1000, 2000] {
                    for extend in [false, true] {
                        let p = FitPoint {
                            w_s: w,
                            k,
                            j,
                            step_ms,
                            extend,
                            ..point()
                        };
                        let expected = Units {
                            explicit: Some((w as u32, k, j, step_ms)),
                            extend,
                            ..Default::default()
                        }
                        .reading(&params(), &ticks, 500000);
                        let actual = readings.at(p, &params(), &ticks, 500000);
                        match (expected, actual) {
                            (Some(a), Some(b)) => {
                                assert!((a.drop_pct - b.drop_pct).abs() < 1e-10);
                                assert_eq!(a.last_price, b.last_price);
                            }
                            (None, None) => {}
                            _ => panic!("cached zone disagrees with direct complete zones"),
                        }
                    }
                }
            }
        }
    }
    // A print arriving after an earlier decision cannot alter its wall-slot reading.
    let wall = FitPoint {
        wall_slots: true,
        ..point()
    };
    let before = readings.at(wall, &params(), &ticks, 500000).unwrap();
    let mut future = ticks;
    future.push(tick(503000, 200.0, Side::Buy));
    let after = Readings::default()
        .at(wall, &params(), &future, 500000)
        .unwrap();
    assert_eq!(before, after);
}

/// Resetting from a logged prior detect can hide an early threshold crossing but cannot earn a hit.
#[test]
fn fire_time_respects_previous_logged_detect() {
    let row = detect(500000, 5.0, 100.0);
    let previous = detect(499000, 5.0, 100.0);
    let ticks = vec![
        tick(0, 105.0, Side::Buy),
        tick(440000, 100.0, Side::Buy),
        tick(502000, 100.0, Side::Buy),
    ];
    let fitted = fit_detect(
        &row,
        &params(),
        &ticks,
        &envelope(500000),
        Some(&previous),
        &[FitPoint {
            w_s: 300,
            k: 1,
            j: 0,
            ..point()
        }],
    );
    assert!(fitted.errors[0].logged_fire2s);
    assert!(!fitted.errors[0].fire2s);
    assert!(!fitted.passes(0));
}

/// Providing or removing a previous log record must never alter causal agreement.
#[test]
fn causal_fire_ignores_logged_prior_detect() {
    let row = detect(500000, 5.0, 100.0);
    let previous = detect(499000, 5.0, 100.0);
    let ticks = vec![
        tick(0, 105.0, Side::Buy),
        tick(440000, 100.0, Side::Buy),
        tick(502000, 100.0, Side::Buy),
    ];
    let points = [FitPoint {
        w_s: 300,
        k: 1,
        j: 0,
        ..point()
    }];
    let a = fit_detect(
        &row,
        &params(),
        &ticks,
        &envelope(500000),
        Some(&previous),
        &points,
    );
    let b = fit_detect(&row, &params(), &ticks, &envelope(500000), None, &points);
    assert_eq!(a.passing, b.passing);
    assert!(!a.errors[0].fire2s);
    assert!(a.errors[0].logged_fire2s);
    assert!(!b.errors[0].logged_fire2s);
}

/// Half a logged decimal passes, while the next hundredth fails and retains its error.
#[test]
fn drop_tolerance_is_half_of_logged_decimal() {
    let ticks = vec![
        tick(0, 105.0, Side::Buy),
        tick(500000, 100.0, Side::Buy),
        tick(502000, 100.0, Side::Buy),
    ];
    let points = [FitPoint {
        k: 1,
        j: 0,
        ..point()
    }];
    for (xpd, expected, truncation) in [
        (4.95, true, true),
        (4.94, false, true),
        (5.04, true, false),
        (5.0 + 5e-10, true, true),
        (5.0 + 2e-9, true, false),
    ] {
        let fit = fit_detect(
            &detect(500000, xpd, 100.0),
            &params(),
            &ticks,
            &envelope(500000),
            None,
            &points,
        );
        assert_eq!(fit.errors[0].drop05, expected);
        assert_eq!(fit.errors[0].truncation, truncation);
        assert!((fit.errors[0].drop_error - (5.0 - xpd).abs()).abs() < 1e-9);
        assert_eq!(fit.errors[0].ask_error_pct, 0.0);
    }
}

/// The widest coverage envelope is refused equally for small and large windows.
#[test]
fn partial_coverage_refuses_not_misses() {
    let row = detect(500000, 4.8, 98.0);
    let fit = fit_detect(
        &row,
        &params(),
        &tape(),
        &Coverage::one((80001, 502000)),
        None,
        &[
            point(),
            FitPoint {
                w_s: 300,
                ..point()
            },
        ],
    );
    assert_eq!(fit.refusal, Some(Refusal::PartialTape));
    assert_eq!(fit.errors.len(), 2);
    assert_eq!(fit.passing, [0]);
    assert!(fit.errors.iter().all(|e| e.drop_error.is_infinite()));
    assert_eq!(
        fit_detect(
            &row,
            &params(),
            &tape(),
            &envelope(500000),
            None,
            &[point()]
        )
        .refusal,
        None
    );
}

/// A slightly inconsistent implied high can still pass all rounding and timing checks.
#[test]
fn infeasible_is_diagnostic_only() {
    let ticks = vec![
        tick(0, 105.0, Side::Buy),
        tick(500000, 100.0, Side::Buy),
        tick(502000, 100.0, Side::Buy),
    ];
    let fit = fit_detect(
        &detect(500000, 5.01, 100.0),
        &params(),
        &ticks,
        &envelope(500000),
        None,
        &[FitPoint {
            k: 1,
            j: 0,
            ..point()
        }],
    );
    assert!(fit.infeasible);
    assert_eq!(fit.refusal, None);
    assert!(fit.passes(0));
}

/// Invalid affine output must stay a miss alongside valid candidates without indexing panics.
#[test]
fn out_of_domain_point_is_miss_not_panic() {
    let points = [
        point(),
        FitPoint { w_s: -1, ..point() },
        FitPoint {
            w_s: 400,
            ..point()
        },
        FitPoint { j: 9, ..point() },
        FitPoint { k: 0, ..point() },
        FitPoint { k: 31, ..point() },
    ];
    let fit = fit_detect(
        &detect(500000, 4.8, 98.0),
        &params(),
        &tape(),
        &envelope(500000),
        None,
        &points,
    );
    assert_eq!(fit.errors.len(), 6);
    assert!(fit.passes(0));
    for i in 1..6 {
        assert!(!fit.passes(i));
        assert!(fit.errors[i].drop_error.is_infinite());
    }
    assert_eq!(FitPoint::grid().count(), 299 * 30 * 7 * 3 * 2 * 4);
}

/// Parameter exclusion applies before any candidate is scored, independent of logged low state.
#[test]
fn price_is_low_strategy_excluded() {
    let p = DropsParams {
        price_is_low: true,
        ..params()
    };
    let fit = fit_detect(
        &detect(500000, 4.8, 98.0),
        &p,
        &tape(),
        &envelope(500000),
        None,
        &[point()],
    );
    assert_eq!(fit.refusal, Some(Refusal::PriceIsLowOn));
    assert_eq!(fit.passing, [0]);
}

/// Generalized cache anchors must preserve every legacy sample-count error on a full tape.
#[test]
fn cached_at_buy_set_equals_legacy_cached_new() {
    let mut row = super::super::tests::row();
    row.deal.buy_set_ms = Some(500000);
    row.deal.drops_detect = Some(crate::db::tuner::ticks::drops::DropsDetect {
        x_price_delta_pct: 4.8,
        price_is_low: false,
    });
    let ticks = tape();
    let legacy = super::super::units::Cached::new(&row, &ticks);
    let generalized = super::super::units::Cached::at(&ticks, 500000, 4.8, 2000);
    for w in [1, 2, 20, 120] {
        for k in [1, 3, 20] {
            for j in [0, 2, 6] {
                assert_eq!(legacy.error(w, k, j), generalized.error(w, k, j));
                let direct = Units {
                    explicit: Some((w as u32 * 2, k, j, 2000)),
                    ..Default::default()
                }
                .reading(&params(), &ticks, 500000)
                .map_or(f64::INFINITY, |r| (r.drop_pct - 4.8).abs());
                assert!(
                    direct == generalized.error(w, k, j)
                        || (direct - generalized.error(w, k, j)).abs() < 1e-10
                );
            }
        }
    }
    let one = super::super::units::Cached::at(&ticks, 500000, 4.8, 1000);
    assert!(one.error(0, 0, 9).is_infinite());
    let u = Units {
        explicit: Some((20, 3, 2, 1000)),
        ..Default::default()
    };
    let direct = (u.reading(&params(), &ticks, 500000).unwrap().drop_pct - 4.8).abs();
    assert!((one.error(20, 3, 2) - direct).abs() < 1e-10);
}
