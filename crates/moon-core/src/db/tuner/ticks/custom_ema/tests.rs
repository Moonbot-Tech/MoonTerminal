//! Independent synthetic oracles for grammar, price smoothing and volume windows.

use super::*;

/// Mixed, unsorted history must keep minute-first duration weighting and ignore touching bars.
#[test]
fn hour_price_mixed_bars_preserves_minute_precedence() {
    let bars = [
        bar(300, 300, 120.0),
        bar(60, 60, 90.0),
        bar(0, 300, 999.0),
        bar(600, 300, 180.0),
        bar(0, 60, 60.0),
        bar(3600, 60, 999.0),
    ];
    // (60*60 + 90*60 + 120*300 + 180*300) / (60+60+300+300).
    assert_eq!(bar_price(&bars, 3599, 3600, None), Some(137.5));
}

/// Rewriting must preserve source bytes outside the selected numeric spans.
#[test]
fn rewrite_changes_only_threshold_digits() {
    let source = " ema ( 60s, 1 ) > +0.25%  aNd Vol(0,5s)<-2.00 ";
    let formula = parse(source).unwrap();
    assert_eq!(
        formula.with_thresholds(source, &[(0, 0.5), (1, -3.0)]),
        " ema ( 60s, 1 ) > 0.5%  aNd Vol(0,5s)<-3 "
    );
}

/// The rewritten source must parse to the same comparisons and written duration families.
#[test]
fn rewrite_roundtrips_parse() {
    let source = "EMA(60,1)>1 AND MAX(5m,1s)<-0.5";
    let formula = parse(source).unwrap();
    let rewritten = parse(&formula.with_thresholds(source, &[(0, -2.5), (1, 0.1)])).unwrap();
    assert_eq!(formula.skeleton(), rewritten.skeleton());
    assert_eq!(
        rewritten
            .thresholds
            .iter()
            .map(|t| t.value)
            .collect::<Vec<_>>(),
        [-2.5, 0.1]
    );
}

/// Sentinels occupy comparison positions but neither expose nor accept knob edits.
#[test]
fn sentinels_not_knobs() {
    let source = "EMA(60,1)>888 AND EMA(60,1)<-1";
    let formula = parse(source).unwrap();
    assert_eq!(
        formula.with_thresholds(source, &[(0, 1.0), (1, -2.0)]),
        "EMA(60,1)>888 AND EMA(60,1)<-2"
    );
    let values = [("SellByCustomEMA".into(), source.into())].into();
    assert_eq!(
        super::super::params::formula_slot_values(&values),
        [("SellByCustomEMA#2".into(), -1.0)].into()
    );
}

/// Same tape and warm window reuse the allocation; a changed window must not reuse it.
#[test]
fn second_series_cache_reuses_only_identical_window() {
    let tape: Arc<[Tick]> = Arc::from([tick(0.0, 100.0, 1.0), tick(10_000.0, 110.0, 2.0)]);
    with_series_tape(&tape, || {
        let first = cached_series(&tape, 0, 10);
        let hit = cached_series(&tape, 0, 10);
        assert!(Arc::ptr_eq(&first, &hit));
        assert_eq!(*hit, SecondSeries::from_ticks(&tape, 0, 10));
        let other = cached_series(&tape, 1, 10);
        assert!(!Arc::ptr_eq(&first, &other));
        assert_eq!(*other, SecondSeries::from_ticks(&tape, 1, 10));
        let formula = parse("EMA(2,1)>0").unwrap();
        assert_eq!(formula.eval(&hit, &[], 10), formula.eval(&first, &[], 10));
    });
}

/// A candle's wick, not its close or minute replacement, sets an hourly breakout level.
#[test]
fn hourly_max_uses_highs_and_hour_price_weights_duration() {
    let series = SecondSeries::from_ticks(&[tick(0.0, 150.0, 1.0)], 0, 8000);
    let mut candle = bar(0, 300, 100.0);
    candle.high = 200.0;
    candle.low = 50.0;
    let bars = [candle, bar(300, 60, 160.0)];
    assert_eq!(bar_price(&bars, 7199, 3600, Some(true)), Some(200.0));
    assert_eq!(bar_price(&bars, 7199, 3600, Some(false)), Some(50.0));
    assert_eq!(bar_price(&bars, 3599, 3600, None), Some(110.0));
    let formula = parse("MAX(1h,1s)>0").unwrap();
    let mut state = FormulaState::new(&formula, &series);
    state.warm_from_bars(&bars, 7199);
    assert_eq!(state.advance_to(7199), Ok(false));
    assert_eq!(state.values, formula.term_values(&series, &bars, 7199));
}

/// Cached missing or known prices persist within a bucket and refresh at its rollover.
#[test]
fn historical_slots_cache_and_roll_over() {
    let series = SecondSeries::from_ticks(&[tick(0.0, 150.0, 1.0)], 0, 11_000);
    let formula = parse("EMA(1h,1s)>0 AND MAX(5m,1s)>0").unwrap();
    let mut state = FormulaState::new(&formula, &series);
    state.warm_from_bars(&[bar(0, 300, 100.0), bar(3600, 300, 200.0)], 7199);
    state.advance_to(7199).unwrap();
    let hour_cached = state.ema[0].historical;
    let minute_cached = state.ema[1].historical;
    let values = state.values.clone();
    state.bars.clear();
    assert_eq!(state.advance_to(7200), Ok(false));
    assert_eq!(state.ema[0].historical, hour_cached);
    assert_eq!(state.ema[1].historical, minute_cached);
    assert_eq!(state.values, values);
    assert_eq!(state.advance_to(10_799), Ok(false));
    assert_ne!(state.ema[0].historical, hour_cached);
    assert_ne!(state.ema[1].historical, minute_cached);
}

/// Equal elapsed durations must still select distinct price algorithms.
#[test]
fn units_keep_their_family() {
    let formula = parse("EMA(120s,2m)>+1% AND EMA(1hour,41hours)<-2").unwrap();
    assert_eq!(
        (formula.terms[0].window_family, formula.terms[0].step_family),
        (UnitFamily::Sec, UnitFamily::Min)
    );
    assert_eq!(formula.terms[1].window_family, UnitFamily::Hour);
    assert_eq!(
        &"EMA(120s,2m)>+1% AND EMA(1hour,41hours)<-2"[formula.thresholds[0].span.clone()],
        "+1"
    );
}

/// Family limits are syntax constraints, including both arguments and volume's finite menu.
#[test]
fn out_of_range_windows_are_unparsed() {
    for src in [
        "EMA(1m,1s)>0",
        "EMA(301s,1s)>0",
        "EMA(42h,1s)>0",
        "EMA(2m,91m)>0",
        "MAX(4m,1s)>0",
        "MIN(300s,1s)>0",
        "BTC(1m,1s)>0",
        "Vol(2m,5s)>0",
        "Vol(5s,0)>0",
    ] {
        assert!(parse(src).is_err(), "{src}");
    }
    for src in [
        "EMA(2m,1s)>0",
        "EMA(41h,1s)>0",
        "MIN(1h,300s)>0",
        "MAX(90m,41h)>0",
        "Vol(0,24h)>0",
    ] {
        assert!(parse(src).is_ok(), "{src}");
    }
}

/// UTC alignment distinguishes a historical minute from a rolling average or a slow EMA.
#[test]
fn minute_price_is_the_minute_n_back() {
    let ticks = (0..360)
        .map(|s| {
            tick(
                f64::from(s * 1000),
                if s < 60 {
                    80.0
                } else if s < 90 {
                    100.0
                } else if s < 120 {
                    140.0
                } else {
                    200.0
                },
                1.0,
            )
        })
        .collect::<Vec<_>>();
    let series = SecondSeries::from_ticks(&ticks, 0, 359);
    assert_eq!(minute_price(&series, 185, 120), Some(120.0));
    assert_eq!(minute_price(&series, 239, 120), Some(200.0));
    let empty_bucket = SecondSeries::from_ticks(&ticks[..120], 0, 359);
    assert_eq!(minute_price(&empty_bucket, 299, 120), Some(120.0));
    assert_eq!(minute_price(&series, 59, 120), None);
}

/// Synthetic closed bars independent of production candle readers.
fn bar(from_s: i64, duration_s: i64, close: f64) -> Bar {
    Bar {
        from_ms: from_s * 1000,
        to_ms: (from_s + duration_s) * 1000,
        open: close,
        high: close,
        low: close,
        close,
    }
}

/// Hour prices weight minute-first closes without sampling the current or a future hour.
#[test]
fn hour_price_from_bars() {
    let formula = parse("EMA(1h,1s)>0").unwrap();
    let series = SecondSeries::from_ticks(&[tick(0.0, 150.0, 1.0)], 0, 8000);
    let mut state = FormulaState::new(&formula, &series);
    state.warm_from_bars(
        &[
            bar(0, 60, 80.0),
            bar(0, 300, 999.0),
            bar(60, 60, 120.0),
            bar(300, 300, 100.0),
            bar(3600, 300, 999.0),
        ],
        3599,
    );
    assert_eq!(state.advance_to(3599), Ok(true));
    assert_eq!(state.values, [Some(50.0)]);
    assert_eq!(state.history_path(), EmaHistoryPath::KlineWarmed);
    assert_eq!(
        state.values,
        formula.term_values(
            &series,
            &[
                bar(0, 60, 80.0),
                bar(0, 300, 999.0),
                bar(60, 60, 120.0),
                bar(300, 300, 100.0),
                bar(3600, 300, 999.0),
            ],
            3599
        )
    );
}

/// Each side of an EMA comparison keeps its own family and independent price.
#[test]
fn ema_mixes_families() {
    let formula = parse("EMA(2m,1s)>20").unwrap();
    let ticks = (0..360)
        .map(|s| {
            tick(
                f64::from(s * 1000),
                if s < 180 { 100.0 } else { 150.0 },
                1.0,
            )
        })
        .collect::<Vec<_>>();
    let series = SecondSeries::from_ticks(&ticks, 0, 359);
    let mut state = FormulaState::new(&formula, &series);
    assert_eq!(state.advance_to(239), Ok(true));
    assert_eq!(state.values, [Some(50.0)]);
    assert_eq!(formula.eval(&series, &[], 239), Ok(true));
}

/// A sharp rise in either bucket touching the last 60 seconds must not raise the level.
#[test]
fn max_minute_excludes_last_60s() {
    let formula = parse("MAX(5m,1s)>0").unwrap();
    let ticks = (0..420)
        .map(|s| {
            tick(
                f64::from(s * 1000),
                if s >= 300 {
                    500.0
                } else {
                    100.0 + (s / 60) as f32 * 10.0
                },
                1.0,
            )
        })
        .collect::<Vec<_>>();
    let series = SecondSeries::from_ticks(&ticks, 0, 419);
    assert_eq!(
        extreme_price(&series, &[], 389, &formula.terms[0]),
        Some(140.0)
    );
}

/// Hour extrema use five-minute candles, regardless of overlapping minute closes.
#[test]
fn min_hour_uses_bars_one_hour_back() {
    let formula = parse("MIN(1h,1s)<0").unwrap();
    let series = SecondSeries::from_ticks(&[tick(0.0, 90.0, 1.0)], 0, 7199);
    let bars = [
        bar(-300, 300, 1.0),
        bar(0, 60, 2.0),
        bar(240, 60, 100.0),
        bar(300, 300, 120.0),
        bar(3600, 300, 3.0),
    ];
    assert_eq!(
        extreme_price(&series, &bars, 7199, &formula.terms[0]),
        Some(120.0)
    );
    let mut state = FormulaState::new(&formula, &series);
    state.warm_from_bars(&bars, 7199);
    assert_eq!(state.advance_to(7199), Ok(true));
    assert!((state.values[0].unwrap() + 25.0).abs() < 1e-10);
    assert_eq!(formula.eval(&series, &bars, 7199), Ok(true));
}

/// Breakout compares the current price to the historical extreme, in percent.
#[test]
fn max_breakout_value() {
    let formula = parse("MAX(5m,1s)>9").unwrap();
    let ticks = (0..420)
        .map(|s| {
            tick(
                f64::from(s * 1000),
                if s >= 360 { 110.0 } else { 100.0 },
                1.0,
            )
        })
        .collect::<Vec<_>>();
    let series = SecondSeries::from_ticks(&ticks, 0, 419);
    let mut state = FormulaState::new(&formula, &series);
    assert_eq!(state.advance_to(389), Ok(true));
    assert!((state.values[0].unwrap() - 10.0).abs() < 1e-10);
}

/// X=0 returns quote units per minute and never divides by a fabricated baseline.
#[test]
fn vol_zero_x_is_plain_average_minute_volume() {
    let formula = parse("Vol(0,5s)>100").unwrap();
    let series = SecondSeries::from_ticks(&[tick(0.0, 10.0, 1.0), tick(5000.0, 20.0, 2.0)], 0, 10);
    assert_eq!(formula.warm_up_s(), 5);
    assert_eq!(vol_term(&series, 5, 0, 5), Some(480.0));
    let mut state = FormulaState::new(&formula, &series);
    assert_eq!(state.advance_to(5), Ok(true));
    assert_eq!(state.values, [Some(480.0)]);
}

/// Tape and newer candles cannot manufacture an hour outside the retained bar reach.
#[test]
fn hour_terms_short_beyond_bar_reach() {
    let formula = parse("EMA(5h,1s)>888").unwrap();
    assert_eq!(formula.warm_up_s(), 6 * 3600);
    let series = SecondSeries::from_ticks(&[tick(-30_000_000.0, 100.0, 1.0)], -30000, 0);
    let mut state = FormulaState::new(&formula, &series);
    state.warm_from_bars(&[bar(-14400, 300, 100.0)], -1);
    assert_eq!(state.advance_to(-1), Err(Refusal::ShortHistory));
}

/// Clipping to the short EMA warm-up must not discard the longer Vol term's history.
#[test]
fn mixed_ema_vol_warm_up_covers_vol_window() {
    use crate::db::tuner::ticks::exit::custom_ema_exit::{CustomEmaClock, first_check_ms};

    let formula = parse("EMA(2s,1s) > 888 AND Vol(1m,5s) > 0").unwrap();
    assert_eq!(formula.warm_up_s(), 60);
    let ticks = (-120..=1)
        .map(|second| tick(f64::from(second * 1_000), 100.0, 1.0))
        .collect::<Vec<_>>();
    let first = first_check_ms(0);
    let from_s = first / 1000 - 1 - i64::from(formula.warm_up_s());
    let series = SecondSeries::from_ticks(&ticks, from_s, 1);
    let mut clock = CustomEmaClock::new(&formula, 0, &[], &series);
    assert_eq!(clock.next_fire(first, &ticks), Some(1_000));
    assert!(!clock.short_history);
}

/// Covered tape must retain its own seed even when conflicting cached bars are available.
#[test]
fn warm_skips_bars_when_tape_covers() {
    let formula = parse("EMA(2s,1s) > 888").unwrap();
    let series = SecondSeries::from_ticks(&[tick(-10_000.0, 100.0, 1.0)], -10, 1);
    let bars = [Bar {
        from_ms: -60_000,
        to_ms: 0,
        open: 999.0,
        high: 999.0,
        low: 999.0,
        close: 999.0,
    }];
    let mut state = FormulaState::new(&formula, &series);
    state.warm_from_bars(&bars, 0);
    assert_eq!(state.ema[0].warmed_from_s, None);
    assert!(state.ema[0].averages.iter().all(Option::is_none));
    assert_eq!(state.advance_to(0), Ok(true));
    assert_eq!(state.history_path(), EmaHistoryPath::Tape);
    assert_eq!(state.values, [Some(0.0)]);
}

/// Clipping between prints must carry the latest known price without counting old volume.
#[test]
fn clipped_series_carries_prior_price_without_volume() {
    let series = SecondSeries::from_ticks(
        &[
            tick(-1_000.0, 100.0, 20.0),
            tick(-2_000.0, 90.0, 10.0),
            tick(1_000.0, 110.0, 1.0),
        ],
        0,
        1,
    );
    assert_eq!(series.last, [Some(100.0), Some(110.0)]);
    assert_eq!(series.quote_vol, [0.0, 110.0]);
    assert_eq!(series.quote_prefix, [0.0, 0.0, 110.0]);
}

/// Sampling a mixed five-minute close would replace minute history and corrupt the EMA seed.
#[test]
fn warm_prefers_minute_bars_over_five_minute() {
    let formula = parse("EMA(120,1) > 0").unwrap();
    let series = SecondSeries::from_ticks(&[tick(300_000.0, 100.0, 1.0)], 300, 360);
    let closes = [100.0, 110.0, 90.0, 120.0, 80.0];
    let mut bars = closes
        .iter()
        .enumerate()
        .map(|(i, &close)| Bar {
            from_ms: i as i64 * 60_000,
            to_ms: (i as i64 + 1) * 60_000,
            open: close,
            high: close,
            low: close,
            close,
        })
        .collect::<Vec<_>>();
    bars.push(Bar {
        from_ms: 0,
        to_ms: 300_000,
        open: 999.0,
        high: 999.0,
        low: 999.0,
        close: 999.0,
    });
    bars.sort_by_key(|bar| (bar.from_ms, bar.to_ms));
    let mut state = FormulaState::new(&formula, &series);
    state.warm_from_bars(&bars, 360);
    let expected = closes[1..].iter().fold(closes[0], |old, close| {
        close + (old - close) * (1.0_f64 - 2.0 / 121.0).powi(60)
    });
    assert_eq!(state.ema[0].warmed_from_s, Some(0));
    assert!((state.ema[0].averages[0].unwrap() - expected).abs() < 1e-10);
}

/// Ordinary filter-bound closes must remain separate from EMA-family FilterCheck exits.
#[test]
fn filtercheck_reason_classifies_ema_vs_filters_bounds() {
    for reason in [
        "Sell by FilterCheck:EMA(60sec, 1sec) = 0.81%",
        "Sell by FilterCheck:Max(30min, 1sec) = 0.33%  EMA(15sec, 1sec) = 0.57%",
        "  sell BY filtercheck:vol(2s,1s) = 2",
        "Sell by FilterCheck:Arb(2s,1s) = 0",
    ] {
        assert!(is_custom_ema_reason(reason), "{reason}");
    }
    for reason in [
        "Sell by FilterCheck: Vol Minute delta (45.64) doesnt match",
        "Sell Price",
        "абвгдежзийклмнопрстуфхцчшщ",
        "EMA(60sec, 1sec)",
    ] {
        assert!(!is_custom_ema_reason(reason), "{reason}");
    }
}
use crate::feed::types::Side;

/// Make a synthetic print without relying on production data or replay helpers.
fn tick(time_ms: f64, price: f32, qty: f32) -> Tick {
    Tick {
        time_ms,
        price,
        qty,
        side: Side::Buy,
    }
}

/// Catch unit scaling, precedence and byte-span drift before threshold rewriting uses them.
#[test]
fn parses_units_connectives_and_precedence() {
    let src = "EMA(1h,1s) > 5 AND EMA(15m,1s) > 3 or EMA(30,1) < -1.1%";
    let formula = parse(src).expect("valid formula");
    assert_eq!(
        formula.expr.iter().map(Vec::len).collect::<Vec<_>>(),
        [2, 1]
    );
    assert_eq!(
        formula
            .terms
            .iter()
            .map(|t| (t.window_s, t.step_s))
            .collect::<Vec<_>>(),
        [(3600, 1), (900, 1), (30, 1)]
    );
    assert_eq!(
        formula
            .thresholds
            .iter()
            .map(|t| t.value)
            .collect::<Vec<_>>(),
        [5.0, 3.0, -1.1]
    );
    assert_eq!(
        formula
            .thresholds
            .iter()
            .map(|t| &src[t.span.clone()])
            .collect::<Vec<_>>(),
        ["5", "3", "-1.1"]
    );
    assert_eq!(formula.longest_window_s(), 3600);
    let src = "\u{2003}eMa(1HOUR,1SEC) < +2.5% aNd vOl(3MIN,5s) > 1e-2";
    let formula = parse(src).expect("case and long units");
    assert_eq!(formula.terms[1].fn_name, FnName::Vol);
    assert_eq!(&src[formula.thresholds[0].span.clone()], "+2.5");
    assert_eq!(&src[formula.thresholds[1].span.clone()], "1e-2");
    assert_eq!(
        parse("EMA(2HOURS,1s) > 1")
            .expect("hours")
            .longest_window_s(),
        7200
    );
}

/// Catch incomplete calls, missing operands and trailing connectives instead of accepting a prefix.
#[test]
fn rejects_malformed() {
    for src in [
        "EMA(1h,1s) >",
        "EMA(1h 1s) > 2",
        "> 2",
        "EMA(1h,1s) > 2 AND",
        "",
        "  ",
        "EMA(1,0) > 2",
        "EMA(0,1) > 2",
        "EMA(1,1) >= 2",
        "EMA(1,1) > NaN",
        "EMA(1,1) > 1e999",
        "EMA(1,1) > 2OR EMA(1,1) > 3",
        "EMA(1,1) > 2 OR(EMA(1,1) > 3)",
        "EMA(4294967295h,1s) > 1",
        "EMA(1s,1s,1s) > 1",
        "EMA(1x,1s) > 1",
        "EMA(1s,1s) > 1 junk",
    ] {
        let error = parse(src).expect_err(src);
        assert!(error.offset <= src.len(), "{src}");
        assert!(src.is_char_boundary(error.offset), "{src}");
    }
}

/// Unsupported cross-market calls must survive parsing and refuse even in a losing OR arm.
#[test]
fn flags_unsupported_terms() {
    let formula = parse("Max(30min,1s) > 1 OR Arb(BinanceF) > 1").expect("opaque term");
    assert_eq!(formula.unsupported(), Some("Arb"));
    assert_eq!(
        (formula.terms[0].window_s, formula.terms[0].step_s),
        (1800, 1)
    );
    assert_eq!((formula.terms[1].window_s, formula.terms[1].step_s), (0, 0));
    let series = SecondSeries::from_ticks(&[tick(0.0, 100.0, 1.0)], 0, 10);
    assert_eq!(formula.eval(&series, &[], 10), Err(Refusal::Unsupported));
    assert_eq!(formula.term_values(&series, &[], 10), [None, None]);
    for name in ["Avg", "MAVG", "BTC", "Unknown"] {
        let formula = parse(&format!("{name}(2s,1s) < 1")).expect("unsupported call");
        assert_eq!(formula.unsupported(), Some(name));
    }
}

/// Replacing recursive EMA with a finite-window average or 1/N weights breaks these oracles.
#[test]
fn ema_definition_pinned_on_synthetic_series() {
    let series = SecondSeries::from_ticks(
        &[
            tick(0.0, 100.0, 1.0),
            tick(1000.0, 100.0, 1.0),
            tick(2000.0, 102.0, 1.0),
            tick(4000.0, 106.0, 1.0),
        ],
        0,
        4,
    );
    // X=3,Y=1: alpha=1/2; prices 100,100,102,102,106 (second 3 is filled).
    // E = 100,100,101,101.5,103.75, so the deviation is (106/103.75-1)*100.
    let expected = (106.0 / 103.75 - 1.0) * 100.0;
    assert!((ema_term(&series, 4, 3, 1).expect("EMA") - expected).abs() < 1e-10);
    // Independent EMA(6) and EMA(2), including the forward-filled second 3.
    let expected = ((2824.0 / 27.0) / (35128.0 / 343.0) - 1.0) * 100.0;
    assert!((ema_term(&series, 4, 6, 2).expect("two-second EMA") - expected).abs() < 1e-10);
    assert!(
        (ema_term(&series, 4, 1, 2).unwrap() - ((2824.0 / 27.0) / 106.0 - 1.0) * 100.0).abs()
            < 1e-10
    );
    assert_eq!(ema_term(&series, 4, 3, 0), None);
    let empty = SecondSeries::from_ticks(&[], 0, 4);
    assert_eq!(ema_term(&empty, 4, 3, 1), None);
}

/// Catch base-volume substitution, inclusive-left bounds and volume forward-filling.
#[test]
fn vol_definition_pinned_on_synthetic_series() {
    let series = SecondSeries::from_ticks(
        &[
            tick(0.0, 1000.0, 1.0),
            tick(1000.0, 10.0, 1.0),
            tick(2000.0, 20.0, 2.0),
            tick(4000.0, 30.0, 1.0),
        ],
        0,
        4,
    );
    // Window (0,4] has quote volume 10+40+0+30=80; two steps average 40.
    assert_eq!(vol_term(&series, 4, 4, 2), Some(30.0 / 40.0));
    assert_eq!(vol_term(&series, 4, 4, 1), Some(30.0 / 20.0));
    assert_eq!(vol_term(&series, 4, 4, 0), None);
    assert_eq!(
        vol_term(&SecondSeries::from_ticks(&[], 0, 4), 4, 4, 2),
        Some(0.0)
    );
    let formula = parse("Vol(5s,5s) < 1.1").expect("volume formula");
    assert_eq!(formula.warm_up_s(), 5);
    let series = SecondSeries::from_ticks(&[tick(0.0, 100.0, 1.0)], 0, 10);
    assert_eq!(formula.eval(&series, &[], 5), Ok(true));
}

/// Sentinels ignore comparison direction but cannot manufacture warm history.
#[test]
fn sentinel_888_true_once_data() {
    let series = SecondSeries::from_ticks(&[tick(0.0, 100.0, 1.0)], 0, 10);
    for src in ["EMA(2,1) > 888", "EMA(2,1) < -888", "Vol(5,5) > 999"] {
        let formula = parse(src).expect("sentinel");
        assert_eq!(formula.eval(&series, &[], 10), Ok(true));
        assert_eq!(formula.eval(&series, &[], 0), Err(Refusal::ShortHistory));
    }
    let formula = parse("EMA(2,1) > 887").expect("ordinary threshold");
    assert_eq!(formula.eval(&series, &[], 10), Ok(false));
    let formula = parse("EMA(2,1) > 0 AND EMA(2,1) < -1 OR EMA(2,1) < 1").expect("OR");
    assert_eq!(formula.eval(&series, &[], 10), Ok(true));
    let formula = parse("EMA(2,1) < 1 AND EMA(2,1) > 1 OR EMA(2,1) > 2").expect("AND");
    assert_eq!(formula.eval(&series, &[], 10), Ok(false));
}

/// Require a price at the warm-up boundary, including a first print inside the held range.
#[test]
fn short_history_refuses() {
    let formula = parse("EMA(2,1) < 1").expect("formula");
    assert_eq!(formula.warm_up_s(), 6);
    let series = SecondSeries::from_ticks(&[tick(1000.0, 100.0, 1.0)], 0, 10);
    assert_eq!(formula.eval(&series, &[], 6), Err(Refusal::ShortHistory));
    assert_eq!(formula.eval(&series, &[], 7), Ok(true));
    assert_eq!(formula.eval(&series, &[], 11), Err(Refusal::ShortHistory));
    let empty = SecondSeries::from_ticks(&[], 0, 10);
    assert_eq!(formula.eval(&empty, &[], 10), Err(Refusal::ShortHistory));
}

/// Future prints, including one millisecond after the evaluated second ends, cannot leak back.
#[test]
fn uses_only_prints_at_or_before_second() {
    let mut ticks = vec![tick(0.0, 100.0, 1.0), tick(3000.0, 104.0, 2.0)];
    let before = SecondSeries::from_ticks(&ticks, 0, 10);
    ticks.push(tick(4001.0, 999.0, 1000.0));
    let after = SecondSeries::from_ticks(&ticks, 0, 10);
    assert_eq!(ema_term(&before, 3, 2, 1), ema_term(&after, 3, 2, 1));
    assert_eq!(vol_term(&before, 3, 2, 1), vol_term(&after, 3, 2, 1));
    let formula = parse("EMA(1,1) < 1 AND Vol(5,5) > 0.5").expect("formula");
    assert_eq!(formula.eval(&before, &[], 3), Err(Refusal::ShortHistory));
    assert_eq!(formula.eval(&after, &[], 3), Err(Refusal::ShortHistory));
    // The final print in each second wins, invalid prices do not contribute volume,
    // and timestamps immediately before the next second still belong to this one.
    let series = SecondSeries::from_ticks(
        &[
            tick(1999.0, 102.0, 1.0),
            tick(1000.0, 100.0, 1.0),
            tick(1500.0, 0.0, 100.0),
            tick(2000.0, 200.0, 1.0),
        ],
        0,
        3,
    );
    assert_eq!(series.last, [None, Some(102.0), Some(200.0), Some(200.0)]);
    assert_eq!(series.quote_vol, [0.0, 202.0, 200.0, 0.0]);
}

/// Reading the bucket at B would leak its opening print into the boundary's decision.
#[test]
fn boundary_reads_only_prints_before_it() {
    let formula = parse("EMA(2,1) > 1").unwrap();
    let mut ticks = vec![tick(0.0, 100.0, 1.0)];
    let series = SecondSeries::from_ticks(&ticks, 0, 11);
    assert_eq!(formula.eval(&series, &[], 9), Ok(false));
    ticks.push(tick(9_999.0, 110.0, 1.0));
    let before = SecondSeries::from_ticks(&ticks, 0, 11);
    assert_eq!(formula.eval(&before, &[], 9), Ok(true));
    ticks.push(tick(10_000.0, 1.0, 1000.0));
    let after = SecondSeries::from_ticks(&ticks, 0, 11);
    assert_eq!(
        formula.term_values(&before, &[], 9),
        formula.term_values(&after, &[], 9)
    );
    assert_eq!(formula.eval(&after, &[], 9), Ok(true));
    assert_eq!(FormulaState::new(&formula, &after).advance_to(9), Ok(true));
}

/// A bogus timestamp span must refuse without allocating an unbounded dense array.
#[test]
fn oversized_range_yields_empty_series() {
    let series = SecondSeries::from_ticks(&[tick(0.0, 100.0, 1.0)], 0, MAX_SERIES_S as i64);
    assert!(series.last.is_empty());
    assert_eq!(
        parse("EMA(2,1) < 1").unwrap().eval(&series, &[], 9),
        Err(Refusal::ShortHistory)
    );
}

/// Invalid quantity must not poison either volume or the last-price path.
#[test]
fn bad_qty_is_skipped() {
    let valid = tick(0.0, 100.0, 2.0);
    let expected = SecondSeries::from_ticks(&[valid], 0, 10);
    let series = SecondSeries::from_ticks(
        &[
            valid,
            tick(1000.0, 1000.0, -1.0),
            tick(2000.0, 1000.0, f32::NAN),
            tick(3000.0, 1000.0, f32::INFINITY),
        ],
        0,
        10,
    );
    assert_eq!(series, expected);
}

/// Missing arms cannot override a determined OR or AND outcome, regardless of order.
#[test]
fn true_or_arm_wins_over_missing_term() {
    let series = SecondSeries::from_ticks(&[tick(0.0, 100.0, 1.0)], 0, 10);
    for (src, expected) in [
        ("EMA(100,1) < 1 OR EMA(2,1) < 1", Ok(true)),
        ("EMA(2,1) < 1 OR EMA(100,1) < 1", Ok(true)),
        ("EMA(100,1) < 1 AND EMA(2,1) > 1", Ok(false)),
        ("EMA(2,1) > 1 AND EMA(100,1) < 1", Ok(false)),
        (
            "EMA(100,1) < 1 AND EMA(2,1) < 1",
            Err(Refusal::ShortHistory),
        ),
        ("EMA(100,1) > 888", Err(Refusal::ShortHistory)),
    ] {
        let formula = parse(src).unwrap();
        assert_eq!(formula.eval(&series, &[], 10), expected, "{src}");
        assert_eq!(
            FormulaState::new(&formula, &series).advance_to(10),
            expected,
            "{src}"
        );
    }
}

/// Empty volume is a defined zero ratio, including through the streaming prefix path.
#[test]
fn zero_volume_window_is_zero_not_short() {
    let series = SecondSeries::from_ticks(&[tick(0.0, 100.0, 0.0)], 0, 10);
    assert_eq!(vol_term(&series, 10, 4, 2), Some(0.0));
    let formula = parse("Vol(5,5) < 1").unwrap();
    assert_eq!(formula.eval(&series, &[], 10), Ok(true));
    assert_eq!(
        FormulaState::new(&formula, &series).advance_to(10),
        Ok(true)
    );
}

/// Changing EMA seeding, sampling phase or prefix endpoints must disagree with the pure oracle.
#[test]
fn incremental_matches_oracle_on_random_series() {
    let mut seed = 417_u64;
    let mut ticks = Vec::new();
    for second in 0..2000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        if second % 7 != 0 {
            ticks.push(tick(
                second as f64 * 1000.0 + 999.0,
                90.0 + (seed >> 32) as u32 as f32 / u32::MAX as f32 * 20.0,
                (seed % 31) as f32,
            ));
        }
    }
    let series = SecondSeries::from_ticks(&ticks, 0, 1999);
    let formula = parse("EMA(60,1) > 0 AND EMA(300,5) < 1 OR Vol(15m,1m) > 1").unwrap();
    let mut state = FormulaState::new(&formula, &series);
    let mut max_error = 0.0_f64;
    for at_s in 0..2000 {
        assert_eq!(
            state.advance_to(at_s),
            formula.eval(&series, &[], at_s),
            "at {at_s}"
        );
        let values = formula.term_values(&series, &[], at_s);
        for (i, oracle) in values.iter().enumerate() {
            if !tape_covers(&series, at_s, &formula.terms[i]) {
                continue;
            }
            let (actual, oracle) = (state.values[i].unwrap(), oracle.unwrap());
            let error = (actual - oracle).abs() / oracle.abs().max(1.0);
            max_error = max_error.max(error);
            assert!(error <= 1e-9, "term {i} at {at_s}: {actual} != {oracle}");
        }
    }
    eprintln!("incremental oracle max relative error: {max_error:.12e}");
}
