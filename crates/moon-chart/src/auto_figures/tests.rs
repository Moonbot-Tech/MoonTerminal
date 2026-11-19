use super::test_bars::{M5_MS, from_hl};
use super::*;
use moon_core::config::auto_figures::{SqueezeParams, SwingParams};

/// A found level reports the money traded on its touching bars and the tolerance it grouped them
/// by: three equal highs at 100, with the touching bars' turnover set apart from the rest, give
/// the sum of exactly those three bars and `tolerance_pct / 100`.
///
/// Breakage: `touch_volume` summing the whole window (or the pivot's neighbours), counting a bar
/// twice, or `tolerance_frac` left in % — the zone band would be a hundred times too wide.
#[test]
fn a_level_reports_its_touch_volume_and_tolerance() {
    let mut bars = from_hl(&[
        (90.0, 85.0),
        (100.0, 95.0),
        (90.0, 85.0),
        (91.0, 86.0),
        (100.0, 95.0),
        (91.0, 86.0),
        (92.0, 87.0),
        (100.0, 95.0),
        (92.0, 87.0),
    ]);
    for b in &mut bars {
        b.quote_volume = 1.0;
    }
    bars[1].quote_volume = 100.0;
    bars[4].quote_volume = 200.0;
    bars[7].quote_volume = 400.0;
    let ctx = DetectCtx {
        tf_ms: M5_MS,
        now_ms: 0.0,
    };
    let swing = DetectorParams::Swing(SwingParams {
        pivot_left: 1,
        pivot_right: 1,
        min_touches: 3,
        tolerance_pct: 0.5,
        ..SwingParams::default()
    });
    let found = detect(&swing, &bars, &ctx);
    let level = found
        .iter()
        .find(|f| f.shape == FoundShape::Horizontal { price: 100.0 })
        .expect("the three highs make a level");
    assert_eq!(level.touch_volume, 700.0);
    assert_eq!(level.tolerance_frac, 0.005);

    // A touch on a bar the input does not hold adds nothing; one bar touched twice counts once.
    let touches = [
        Touch {
            t_ms: bars[1].t_open_ms,
            price: 100.0,
        },
        Touch {
            t_ms: bars[1].t_open_ms,
            price: 95.0,
        },
        Touch {
            t_ms: bars[8].t_open_ms + 1.0,
            price: 92.0,
        },
    ];
    assert_eq!(touch_volume(&bars, &touches), 100.0);
}

/// A squeeze reports its LEVEL's tolerance — the band drawn around the level — not its sloped
/// half's. Breakage: the trend tolerance reported instead, drawing the zone at the line's width.
#[test]
fn a_squeeze_reports_its_level_tolerance() {
    let p = SqueezeParams::default();
    assert_ne!(p.level.tolerance_pct, p.trend_tolerance_pct);
    // The default squeeze's search on the committed bench finds formations (the squeeze module's
    // own bench test pins that); each must carry the level's band.
    let params = DetectorParams::Squeeze(SqueezeParams {
        level: SwingParams {
            lookback_bars: 2_000,
            ..p.level
        },
        ..p
    });
    let mut formations = 0;
    for kind in [1u32, 5] {
        let bars = test_bars::ace(kind);
        let ctx = DetectCtx {
            tf_ms: i64::from(kind) * 60_000,
            now_ms: bars.last().map_or(0.0, |b| b.t_open_ms),
        };
        for f in detect(&params, &bars, &ctx) {
            assert_eq!(f.tolerance_frac, f64::from(p.level.tolerance_pct) / 100.0);
            formations += 1;
        }
    }
    assert!(formations > 0, "the bench holds a squeeze");
}

/// Breakage: a detector missing from the table makes `detect` silently return nothing for a whole
/// kind; a duplicate row makes the second one unreachable.
#[test]
fn registry_has_exactly_one_row_per_kind() {
    for kind in DetectorKind::ALL {
        let rows = REGISTRY.iter().filter(|d| d.kind() == kind).count();
        assert_eq!(rows, 1, "{kind:?} must have exactly one registry row");
    }
    assert_eq!(REGISTRY.len(), DetectorKind::ALL.len());
}

/// `detect` must reach the detector the parameters name: three equal highs are a `Swing` level,
/// and the same bars handed `PeriodExtreme` parameters give the day's high and low instead.
///
/// Breakage: dispatching by position instead of by kind.
#[test]
fn detect_dispatches_by_the_parameters_kind() {
    let bars = from_hl(&[
        (90.0, 85.0),
        (100.0, 95.0),
        (90.0, 85.0),
        (91.0, 86.0),
        (100.0, 95.0),
        (91.0, 86.0),
        (92.0, 87.0),
        (100.0, 95.0),
        (92.0, 87.0),
    ]);
    let ctx = DetectCtx {
        tf_ms: M5_MS,
        now_ms: 0.0,
    };
    let swing = DetectorParams::Swing(SwingParams {
        pivot_left: 1,
        pivot_right: 1,
        min_touches: 3,
        tolerance_pct: 0.1,
        ..SwingParams::default()
    });
    let found = detect(&swing, &bars, &ctx);
    assert!(
        found
            .iter()
            .any(|f| f.shape == FoundShape::Horizontal { price: 100.0 }),
        "{found:?}"
    );
    let day = DetectorKind::PeriodExtreme.default_params();
    let shapes: Vec<FoundShape> = detect(&day, &bars, &ctx).iter().map(|f| f.shape).collect();
    assert_eq!(
        shapes,
        vec![
            FoundShape::Horizontal { price: 100.0 },
            FoundShape::Horizontal { price: 85.0 }
        ]
    );
}

/// A line is linear in time and refuses to hand out a non-positive price.
///
/// Breakage: interpolating by bar index, dividing by a zero span, or returning a negative price a
/// caller then divides by.
#[test]
fn line_price_is_linear_in_time_and_positive() {
    assert_eq!(line_price_at(0.0, 100.0, 10.0, 110.0, 5.0), Some(105.0));
    assert_eq!(line_price_at(0.0, 100.0, 10.0, 110.0, 20.0), Some(120.0));
    assert_eq!(line_price_at(0.0, 100.0, 0.0, 110.0, 5.0), None);
    assert_eq!(line_price_at(0.0, 100.0, 10.0, 50.0, 30.0), None);
    assert_eq!(line_price_at(0.0, f64::NAN, 10.0, 50.0, 3.0), None);
    let squeeze = FoundShape::Squeeze {
        level: 100.0,
        t0_ms: 0.0,
        p0: 90.0,
        t1_ms: 10.0,
        p1: 95.0,
        pressed: false,
    };
    assert_eq!(squeeze.price_at(1e12), Some(100.0));
}

/// Empty and too-short input never panics, and finds nothing where one bar cannot answer.
///
/// One bar is not "too short" for every kind. The pivot detectors need neighbours and must find
/// nothing. A calendar, profile or round-price detector legitimately answers from one bar: the bar
/// at Monday 00:00 is the day's and the week's first bar (its high and low, its open, its profile),
/// and a close always has round prices around it. For those every found price must be a positive
/// finite number.
///
/// Breakage: an unchecked index on an empty or 1-bar slice inside any detector, or a pivot detector
/// inventing a level from a single bar.
#[test]
fn every_detector_survives_empty_and_tiny_input() {
    let ctx = DetectCtx {
        tf_ms: M5_MS,
        now_ms: 0.0,
    };
    let one = from_hl(&[(100.0, 99.0)]);
    let nan = vec![test_bars::bar(0, f32::NAN, f32::NAN, f32::NAN, f32::NAN); 20];
    for kind in DetectorKind::ALL {
        let params = kind.default_params();
        assert!(detect(&params, &[], &ctx).is_empty(), "{kind:?} on nothing");
        assert!(
            detect(&params, &nan, &ctx).is_empty(),
            "{kind:?} on NaN bars"
        );
        let found = detect(&params, &one, &ctx);
        match kind {
            DetectorKind::Swing | DetectorKind::Trend | DetectorKind::Squeeze => {
                assert!(found.is_empty(), "{kind:?} on one bar");
            }
            DetectorKind::PeriodExtreme
            | DetectorKind::VolumeProfile
            | DetectorKind::PeriodLevel
            | DetectorKind::RoundLevels => {
                for f in &found {
                    let p = f.shape.price_at(ctx.now_ms);
                    assert!(
                        p.is_some_and(|p| p.is_finite() && p > 0.0),
                        "{kind:?}: {f:?}"
                    );
                }
            }
        }
    }
}

/// The ATR is the simple mean of the last `ATR_PERIOD` true ranges, read from the last
/// `ATR_PERIOD + 1` valid bars only, an unreadable bar skipped.
///
/// By hand: bars of range 98..102 closing at 100 have a true range of 4. The newest bar spans
/// 104..106 after a close of 100, so its true range is 106 − 100 = 6. Over 16 bars the last 14 true
/// ranges are 13 × 4 + 6 = 58: ATR = 58 / 14. Bar 1's huge high would add to the sum if one bar too
/// many were read. With 14 bars there are only 13 true ranges: no ATR.
///
/// Breakage: Wilder smoothing or an all-bars mean, an off-by-one in the bars read, the true range
/// without the previous close, or a NaN bar read as a range.
#[test]
fn atr_is_the_simple_mean_of_the_last_true_ranges() {
    use moon_core::config::auto_figures::ATR_PERIOD;
    let mut bars: Vec<ChartCandle> = (0..15)
        .map(|i| test_bars::bar(i, 100.0, 102.0, 98.0, 100.0))
        .collect();
    bars[1].high = 1_000.0;
    bars.push(test_bars::bar(15, 105.0, 106.0, 104.0, 105.0));
    assert_eq!(atr(&bars, ATR_PERIOD), Some(58.0 / 14.0));

    let mut with_nan = bars.clone();
    with_nan.insert(
        14,
        test_bars::bar(14, f32::NAN, f32::NAN, f32::NAN, f32::NAN),
    );
    assert_eq!(atr(&with_nan, ATR_PERIOD), Some(58.0 / 14.0));

    assert_eq!(atr(&bars[..14], ATR_PERIOD), None);
    assert_eq!(atr(&bars, 0), None);
    let flat: Vec<ChartCandle> = (0..20)
        .map(|i| test_bars::bar(i, 1.0, 1.0, 1.0, 1.0))
        .collect();
    assert_eq!(atr(&flat, ATR_PERIOD), None, "no range, no tolerance");
}

/// Under `ToleranceUnit::Atr` a level groups its pivots by the ATR band and reports that band.
///
/// By hand: 21 bars of range 99..101 closing at 100, with pivot highs of 102.0, 102.5 and 102.8 at
/// bars 4, 10 and 16. A bar's true range is `max(high, 100) − min(low, 100)` = high − 99: 2 for a
/// plain bar, 3.5 and 3.8 for the pivots at 10 and 16. The last 14 (bars 7..=20) sum to
/// 12 × 2 + 3.5 + 3.8 = 31.3, so ATR = 31.3 / 14 ≈ 2.236; half an ATR over the close of 100 is a
/// band of ≈ 1.118 %, wide enough to take all three highs (102.8 is 0.78 % above 102.0) into one
/// level at their mean 102.4333…, while 0.25 % takes none of them together.
///
/// Breakage: the ATR share read as % (a band a hundred times narrower), the ATR computed over the
/// lookback window instead of the whole input, or a silent fallback to `tolerance_pct` when there
/// is no ATR.
#[test]
fn an_atr_tolerance_groups_by_the_atr_band() {
    use moon_core::config::auto_figures::ToleranceUnit;
    let mut bars: Vec<ChartCandle> = (0..21)
        .map(|i| test_bars::bar(i, 100.0, 101.0, 99.0, 100.0))
        .collect();
    for (i, h) in [(4, 102.0), (10, 102.5), (16, 102.8)] {
        bars[i].high = h;
    }
    let ctx = DetectCtx {
        tf_ms: M5_MS,
        now_ms: 0.0,
    };
    let params = |unit, lookback_bars, min_touches, tolerance_pct| {
        DetectorParams::Swing(SwingParams {
            lookback_bars,
            pivot_left: 1,
            pivot_right: 1,
            min_touches,
            tolerance_unit: unit,
            tolerance_pct,
            tolerance_atr: 0.5,
            ..SwingParams::default()
        })
    };
    let expected_frac = 0.5 * (31.3 / 14.0) / 100.0;
    let found = detect(&params(ToleranceUnit::Atr, 18, 3, 0.25), &bars, &ctx);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!((found[0].tolerance_frac - expected_frac).abs() < 1e-6);
    let FoundShape::Horizontal { price } = found[0].shape else {
        panic!("a level");
    };
    assert!(
        (price - (102.0 + 102.5 + 102.8) / 3.0).abs() < 1e-4,
        "{price}"
    );
    assert!(detect(&params(ToleranceUnit::Pct, 18, 3, 0.25), &bars, &ctx).is_empty());

    // A 10-bar lookback holds only the pivot at 16, yet its band is still the ATR of all 21 bars:
    // the ATR is not cut to the window (a 10-bar window has no 14-bar ATR at all).
    let found = detect(&params(ToleranceUnit::Atr, 10, 1, 0.25), &bars, &ctx);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!((found[0].tolerance_frac - expected_frac).abs() < 1e-6);

    // Fourteen bars have no ATR: nothing, although the same row in % finds the 102.5 / 102.8 pair.
    let short = &bars[7..];
    assert!(!detect(&params(ToleranceUnit::Pct, 18, 2, 1.5), short, &ctx).is_empty());
    assert!(detect(&params(ToleranceUnit::Atr, 18, 2, 1.5), short, &ctx).is_empty());
}

/// The ATR of the committed bench, written out independently of `atr`.
fn bench_atr(bars: &[ChartCandle]) -> f64 {
    let n = bars.len();
    let sum: f64 = (n - 14..n)
        .map(|i| {
            let pc = f64::from(bars[i - 1].close);
            f64::from(bars[i].high).max(pc) - f64::from(bars[i].low).min(pc)
        })
        .sum();
    sum / 14.0
}

/// On the real `chart-ace` candles every pivot detector in ATR mode reports the band of THIS
/// run's ATR — `share × ATR / last close`, the squeeze its LEVEL's share — and finds levels at all.
///
/// Breakage: a stale or per-window ATR, the squeeze's level band taken from the line's share, or
/// the ATR unit ignored by one of the three detectors.
#[test]
fn bench_atr_tolerances_follow_the_runs_atr() {
    use moon_core::config::auto_figures::{ToleranceUnit, TrendParams};
    let all = test_bars::ace(5);
    // 1 000 bars: the last 600 hold trend lines and levels, but no squeeze under the default
    // touches; a longer stretch holds all three, so every case below is checked, none skipped.
    let bars = &all[all.len() - 1_000..];
    let close = f64::from(bars[bars.len() - 1].close);
    let atr = bench_atr(bars);
    let ctx = DetectCtx {
        tf_ms: 300_000,
        now_ms: 0.0,
    };
    let swing = SwingParams {
        lookback_bars: 1_000,
        tolerance_unit: ToleranceUnit::Atr,
        ..SwingParams::default()
    };
    let trend = TrendParams {
        lookback_bars: 1_000,
        tolerance_unit: ToleranceUnit::Atr,
        ..TrendParams::default()
    };
    let squeeze = SqueezeParams {
        level: swing,
        ..SqueezeParams::default()
    };
    let cases = [
        (DetectorParams::Swing(swing), swing.tolerance_atr),
        (DetectorParams::Trend(trend), trend.tolerance_atr),
        (
            DetectorParams::Squeeze(squeeze),
            squeeze.level.tolerance_atr,
        ),
    ];
    for (params, share) in cases {
        let expected = f64::from(share) * atr / close;
        let found = detect(&params, bars, &ctx);
        // Every detector, not only the swing: an empty result would pass the band check below
        // vacuously, and a detector that ignored the ATR unit and found nothing would go unseen.
        assert!(
            !found.is_empty(),
            "{:?}: the bench holds ATR figures",
            params.kind()
        );
        for f in &found {
            assert!(
                (f.tolerance_frac - expected).abs() <= expected * 1e-9,
                "{:?}: {} vs {expected}",
                params.kind(),
                f.tolerance_frac
            );
        }
    }
}

/// On the real `chart-ace` candles each figure's `touch_volume` is the turnover of its distinct
/// touching bars, looked up here by a plain scan; and "strongest by volume" puts the side's largest
/// one first.
///
/// Breakage: a bar touched twice (a squeeze's level and line on one bar) counted twice, the volume
/// of a neighbour bar, or the `Volume` basis ranking by the touch count.
#[test]
fn bench_touch_volume_counts_each_touching_bar_once() {
    use super::take::select;
    use moon_core::config::auto_figures::{StrengthBasis, Take, TakeOrder, TakeSide, TrendParams};
    let bars = test_bars::ace(5);
    let ctx = DetectCtx {
        tf_ms: 300_000,
        now_ms: 0.0,
    };
    let rows = [
        DetectorParams::Swing(SwingParams {
            lookback_bars: 2_000,
            ..SwingParams::default()
        }),
        DetectorParams::Trend(TrendParams {
            lookback_bars: 2_000,
            ..TrendParams::default()
        }),
        DetectorParams::Squeeze(SqueezeParams {
            level: SwingParams {
                lookback_bars: 2_000,
                ..SwingParams::default()
            },
            ..SqueezeParams::default()
        }),
    ];
    let price = f64::from(bars[bars.len() - 1].close);
    let mut checked = 0;
    for params in rows {
        let found = detect(&params, &bars, &ctx);
        for f in &found {
            let mut times: Vec<f64> = f.touches.iter().map(|t| t.t_ms).collect();
            times.sort_by(f64::total_cmp);
            times.dedup();
            let by_scan: f64 = times
                .iter()
                .map(|&t| {
                    let b = bars.iter().find(|b| b.t_open_ms == t).expect("touch bar");
                    f64::from(b.quote_volume)
                })
                .sum();
            assert_eq!(f.touch_volume, by_scan, "{:?}", params.kind());
            if f.touch_volume > 0.0 {
                checked += 1;
            }
        }
        let take = Take {
            count: 1,
            side: TakeSide::Above,
            order: TakeOrder::Strongest,
            strength: StrengthBasis::Volume,
        };
        let above: Vec<&Found> = found
            .iter()
            .filter(|f| f.shape.price_at(0.0).is_some_and(|p| p >= price))
            .collect();
        if let Some(&top) = select(&found, &take, price, 0.0).first() {
            let max = above.iter().map(|f| f.touch_volume).fold(0.0, f64::max);
            assert_eq!(found[top].touch_volume, max);
        }
    }
    assert!(checked > 10, "enough figures with turnover: {checked}");
}
