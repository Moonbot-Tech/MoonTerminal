use super::*;
use crate::feed::types::Side;

const T0: i64 = 1_790_000_000_000; // a multiple of STEP_MS and of a minute

/// Clamping NaN during anchoring must not overwrite the report when later BTC bars are coarse.
#[test]
fn an_unanswered_point_stays_unanswered_after_the_anchor() {
    let mut btc = flat_bars(T0 - 60 * MINUTE_MS, T0, 100.0);
    btc.push(Bar {
        from_ms: T0,
        to_ms: T0 + CANDLE_MS,
        open: 100.0,
        high: 110.0,
        low: 90.0,
        close: 100.0,
    });
    let snapshot = Deltas {
        btc1m: 0.7,
        ..Deltas::default()
    };
    let track = DeltaTrack::build(TrackInputs {
        coin_bars: &[],
        ticks: &[tick(T0, 100.0)],
        covered: &[(T0, T0 + CANDLE_MS + STEP_MS)],
        btc_bars: &btc,
        eval: (T0, T0 + CANDLE_MS + STEP_MS),
        anchor: Some((T0, &snapshot)),
    })
    .unwrap();
    assert!(track.is_live(DeltaField::Btc1m));
    assert_eq!(track.apply(T0, &snapshot).btc1m, 0.7);
    assert_eq!(track.value(T0 + CANDLE_MS, DeltaField::Btc1m), None);
    assert_eq!(track.apply(T0 + CANDLE_MS, &snapshot).btc1m, 0.7);
}

fn tick(t_ms: i64, price: f32) -> Tick {
    Tick {
        time_ms: t_ms as f64,
        price,
        qty: 1.0,
        side: Side::Buy,
    }
}

/// Minute bars at a flat price from `from` to `to`.
fn flat_bars(from: i64, to: i64, price: f64) -> Vec<Bar> {
    (from..to)
        .step_by(MINUTE_MS as usize)
        .map(|t| Bar {
            from_ms: t,
            to_ms: t + MINUTE_MS,
            open: price,
            high: price,
            low: price,
            close: price,
        })
        .collect()
}

fn day_of_bars() -> Vec<Bar> {
    flat_bars(T0 - LOOKBACK_MS - CANDLE_MS, T0, 100.0)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// A track without an anchor, over one covered stretch.
fn raw(bars: &[Bar], ticks: &[Tick], covered: (i64, i64), eval: (i64, i64)) -> DeltaTrack {
    DeltaTrack::build(TrackInputs {
        coin_bars: bars,
        ticks,
        covered: &[covered],
        btc_bars: &[],
        eval,
        anchor: None,
    })
    .unwrap()
}

#[test]
fn a_delta_is_the_range_of_what_printed_before_the_boundary() {
    let ticks = [tick(T0 + 1_000, 100.0), tick(T0 + 7_000, 110.0)];
    let track = raw(&day_of_bars(), &ticks, (T0, T0 + 60_000), (T0, T0 + 60_000));
    // The print at +7 s joins at the next boundary, +10 s — not in the bucket it printed in.
    assert_eq!(track.value(T0 + 9_999, DeltaField::D1m), Some(0.0));
    let d1m = track.value(T0 + 10_000, DeltaField::D1m).unwrap();
    assert!(close(d1m, 10.0), "{d1m}");
    for field in [DeltaField::D3h, DeltaField::D24h] {
        assert!(
            close(track.value(T0 + 10_000, field).unwrap(), 10.0),
            "{field:?}"
        );
    }
}

#[test]
fn a_candle_window_reaches_one_candle_past_its_name() {
    // A spike to 120 in a bar that ended 19 minutes before T0: inside d15m's reach (20 min), and
    // out of it once the reach has passed it.
    let mut bars = day_of_bars();
    for b in &mut bars {
        if b.to_ms == T0 - 19 * MINUTE_MS {
            b.high = 120.0;
        }
    }
    let ticks = [tick(T0 + 1_000, 100.0)];
    let track = raw(
        &bars,
        &ticks,
        (T0, T0 + 5 * MINUTE_MS),
        (T0, T0 + 5 * MINUTE_MS),
    );
    assert!(close(track.value(T0, DeltaField::D15m).unwrap(), 20.0));
    assert!(close(track.value(T0, DeltaField::D5m).unwrap(), 0.0));
    assert!(close(
        track.value(T0 + MINUTE_MS, DeltaField::D15m).unwrap(),
        0.0
    ));
    // Still inside d1h, and the long ones never read below it.
    for field in [DeltaField::D1h, DeltaField::D3h, DeltaField::D24h] {
        assert!(
            close(track.value(T0 + MINUTE_MS, field).unwrap(), 20.0),
            "{field:?}"
        );
    }
}

/// "d3h" and "d24h" read closed candles only, younger than their window by the last close: a
/// spike in a bar that ended 4 h 4 min before T0 — whose last close was 200 s before it — is out
/// of d3h, where a window reaching 4 h 5 min back from the moment took it.
#[test]
fn the_long_ranges_read_closed_candles_only() {
    let mut bars = day_of_bars();
    for b in &mut bars {
        if b.to_ms == T0 - 4 * 60 * MINUTE_MS - 4 * MINUTE_MS {
            b.high = 120.0;
        }
    }
    let ticks = [tick(T0 + 1_000, 100.0)];
    let track = raw(&bars, &ticks, (T0, T0 + 60_000), (T0, T0 + 60_000));
    assert!(close(track.value(T0, DeltaField::D3h).unwrap(), 0.0));
    assert!(close(track.value(T0, DeltaField::D24h).unwrap(), 20.0));
}

#[test]
fn a_window_is_read_over_whatever_history_it_has() {
    // Six hours of bars, with a hole in them: every window answers off what is there, and the
    // stamp check says how much of each it covered.
    let mut bars = flat_bars(T0 - 6 * 60 * MINUTE_MS, T0, 100.0);
    bars.retain(|b| !(T0 - 30 * MINUTE_MS..T0 - 20 * MINUTE_MS).contains(&b.from_ms));
    bars[0].low = 95.0;
    let ticks = [tick(T0 + 1_000, 105.0)];
    let snapshot = Deltas {
        d1m: 1.0,
        d5m: 1.0,
        d15m: 1.0,
        d1h: 1.0,
        d3h: 1.0,
        d24h: 12.0,
        ..Deltas::default()
    };
    let track = DeltaTrack::build(TrackInputs {
        coin_bars: &bars,
        ticks: &ticks,
        covered: &[(T0, T0 + 60_000)],
        btc_bars: &[],
        eval: (T0, T0 + 60_000),
        anchor: Some((T0 + 5_000, &snapshot)),
    })
    .unwrap();
    for field in [DeltaField::D1h, DeltaField::D24h] {
        assert!(track.is_live(field), "{field:?}");
    }
    let stamp = track.stamp();
    let d24h = stamp.coverage[DeltaField::D24h.index()];
    assert!(
        d24h > 0.2 && d24h < 0.25,
        "six hours of twenty-five: {d24h}"
    );
    let d1h = stamp.coverage[DeltaField::D1h.index()];
    assert!(d1h > 0.8 && d1h < 0.9, "a ten-minute hole in 65: {d1h}");
    // The six hours' range against the report's twelve: the error before the anchor, negative
    // where the history saw the narrower move. The 105 printed in the open candle, which reaches
    // d24h only through d1h: the closed candles' 100 / 95 is the wider of the two.
    let range = (100.0 / 95.0 - 1.0) * 100.0;
    let error = stamp.error[DeltaField::D24h.index()].unwrap();
    assert!(close(error, range - 12.0), "{error}");
    assert!(close(track.apply(T0 + 5_000, &snapshot).d24h, 12.0));
}

#[test]
fn nothing_evaluated_is_no_track() {
    assert!(
        DeltaTrack::build(TrackInputs {
            coin_bars: &[],
            ticks: &[],
            covered: &[(T0, T0 + 30_000)],
            btc_bars: &[],
            eval: (T0, T0 + 30_000),
            anchor: None,
        })
        .is_none()
    );
}

#[test]
fn the_anchor_puts_the_track_on_the_report_at_its_stamp() {
    let ticks = [tick(T0 + 1_000, 100.0), tick(T0 + 12_000, 104.0)];
    // The report says 1 % on d1m at +5 s (the evaluation says 0) and never filled d15m.
    let snapshot = Deltas {
        d1m: 1.0,
        d5m: 0.5,
        d15m: 0.0,
        d1h: 2.0,
        d3h: 3.0,
        d24h: 9.0,
        ..Deltas::default()
    };
    let track = DeltaTrack::build(TrackInputs {
        coin_bars: &day_of_bars(),
        ticks: &ticks,
        covered: &[(T0, T0 + 60_000)],
        btc_bars: &[],
        eval: (T0, T0 + 60_000),
        anchor: Some((T0 + 5_000, &snapshot)),
    })
    .unwrap();
    let at_stamp = track.apply(T0 + 5_000, &snapshot);
    for field in [DeltaField::D1m, DeltaField::D1h, DeltaField::D24h] {
        assert!(close(field.of(&at_stamp), field.of(&snapshot)), "{field:?}");
    }
    // After the 4 % print the track moved by 4 from where the report stood.
    let later = track.apply(T0 + 15_000, &snapshot);
    assert!(
        close(later.d1m, 5.0) && close(later.d24h, 13.0),
        "{later:?}"
    );
    assert!(
        close(later.d15m, 0.0),
        "a field the report never filled: {later:?}"
    );
    assert!(!track.is_live(DeltaField::D15m));
    // Signed: the evaluation saw 0 where the report said 1.
    assert_eq!(track.stamp().error[DeltaField::D1m.index()], Some(-1.0));
}

#[test]
fn an_anchor_the_track_does_not_reach_is_no_track() {
    let ticks = [tick(T0 + 1_000, 100.0)];
    let snapshot = Deltas {
        d1m: 1.0,
        ..Deltas::default()
    };
    // Stamped before the tape begins: nothing to put the track on the report with.
    assert!(
        DeltaTrack::build(TrackInputs {
            coin_bars: &day_of_bars(),
            ticks: &ticks,
            covered: &[(T0, T0 + 60_000)],
            btc_bars: &[],
            eval: (T0, T0 + 60_000),
            anchor: Some((T0 - 60_000, &snapshot)),
        })
        .is_none()
    );
    // And a deal the report never stamped gets none either.
    let mut deal = crate::db::tuner::ticks::tests::deal();
    deal.kind = "MoonHook".into();
    deal.buy_set_ms = None;
    assert_eq!(snapshot_ms(&deal), None);
}

#[test]
fn a_bar_under_the_tape_is_not_history() {
    // A bar overlapping the tape carries prints from after the moment it would be read at.
    let mut bars = day_of_bars();
    bars.push(Bar {
        from_ms: T0,
        to_ms: T0 + MINUTE_MS,
        open: 100.0,
        high: 150.0,
        low: 100.0,
        close: 100.0,
    });
    let ticks = [tick(T0 + 1_000, 100.0)];
    let track = raw(&bars, &ticks, (T0, T0 + MINUTE_MS), (T0, T0 + MINUTE_MS));
    assert!(close(
        track.value(T0 + 55_000, DeltaField::D1h).unwrap(),
        0.0
    ));
}

#[test]
fn the_track_is_evaluated_only_where_the_models_read_it() {
    let ticks = [tick(T0 + 1_000, 100.0)];
    let track = raw(
        &day_of_bars(),
        &ticks,
        (T0, T0 + 60 * MINUTE_MS),
        (T0 + MINUTE_MS, T0 + 2 * MINUTE_MS),
    );
    assert!(track.value(T0 + 30_000, DeltaField::D1m).is_none());
    assert!(track.value(T0 + MINUTE_MS, DeltaField::D1m).is_some());
    assert!(track.value(T0 + 3 * MINUTE_MS, DeltaField::D1m).is_none());
}

/// Replacing the smoothed range with adjacent bucket moves loses spikes and smooth decay.
#[test]
fn last5s_rises_fast_and_decays_smoothly() {
    assert!(close(coin::last5s_step(1.0, Some(0.5)), 0.9));
    assert!(close(coin::last5s_step(0.5, Some(1.0)), 1.0));
    assert!(close(coin::last5s_step(1.0, None), 0.98));
    let mut ticks = vec![tick(T0 + 100, 100.0), tick(T0 + 4_900, 101.0)];
    ticks.extend((5_100..20_000).step_by(250).map(|ms| tick(T0 + ms, 101.0)));
    let track = raw(&day_of_bars(), &ticks, (T0, T0 + 25_000), (T0, T0 + 25_000));
    assert!(close(
        track.value(T0 + 5_000, DeltaField::D5s).unwrap(),
        1.0
    ));
    let later = track.value(T0 + 20_000, DeltaField::D5s).unwrap();
    // The border's 100 price survives through +10 s; forty occupied slots then have d5 = 0.
    assert!(close(later, 0.8f64.powi(40)), "{later}");
    let empty = track.value(T0 + 25_000, DeltaField::D5s).unwrap();
    assert!(close(empty, later * 0.98f64.powi(20)), "{empty}");
    // The left-border record matters even when it is older than the nominal window.
    let border = raw(
        &[],
        &[tick(T0 - 10_000, 100.0), tick(T0 + 5_000, 101.0)],
        (T0, T0 + 10_000),
        (T0, T0 + 10_000),
    );
    assert!(close(
        border.value(T0 + 10_000, DeltaField::D5s).unwrap(),
        0.98f64.powi(19)
    ));
    // An extreme one millisecond before the border is already outside the range.
    let older = raw(
        &[],
        &[
            tick(T0 - 10_001, 80.0),
            tick(T0 - 10_000, 100.0),
            tick(T0 + 5_000, 101.0),
        ],
        (T0, T0 + 10_000),
        (T0, T0 + 10_000),
    );
    // The print enters at +5250, then nineteen quiet slots lead to the track's +10000 point.
    let expected = (0.8 * 25.0 * 0.98f64.powi(59) + 0.2) * 0.98f64.powi(19);
    assert!(close(
        older.value(T0 + 10_000, DeltaField::D5s).unwrap(),
        expected
    ));
}

/// Reading a print at its own timestamp would give D5s future information other fields lack.
#[test]
fn last5s_excludes_a_print_at_the_boundary_until_the_next_slot() {
    let series = series::Series::new(&[], &[tick(T0 - 100, 100.0), tick(T0, 101.0)], &[]);
    let mut eval = coin::CoinEval::new(&series);
    assert!(close(eval.at(T0)[6].0.unwrap(), 0.0));
    assert!(close(eval.at(T0 + 250)[6].0.unwrap(), 1.0));
}

/// Empty cadence slots must decay in closed form even across a long coverage gap.
#[test]
fn last5s_jumps_a_quiet_stretch_in_one_step() {
    let end = T0 + 2_505_250;
    let series = series::Series::new(&[], &[tick(T0, 100.0), tick(T0 + 5_000, 101.0)], &[]);
    let mut eval = coin::CoinEval::new(&series);
    assert!(close(eval.at(T0 + 5_250)[6].0.unwrap(), 1.0));
    let jumped = eval.at(end)[6].0.unwrap();
    let expected = 0.98f64.powi(10_000);
    assert!(
        (jumped / expected - 1.0).abs() < 1e-12,
        "{jumped} vs {expected}"
    );
}

/// Inspecting only the first and last items misses a coarse bar inside the minute.
#[test]
fn btc1m_with_a_coarse_bar_in_the_middle_is_unanswered() {
    let bars = [
        Bar {
            from_ms: T0 - 60_000,
            to_ms: T0 - 50_000,
            open: 100.0,
            high: 100.0,
            low: 100.0,
            close: 100.0,
        },
        Bar {
            from_ms: T0 - 330_000,
            to_ms: T0 - 30_000,
            open: 100.0,
            high: 110.0,
            low: 90.0,
            close: 100.0,
        },
        Bar {
            from_ms: T0 - 30_000,
            to_ms: T0,
            open: 100.0,
            high: 100.0,
            low: 100.0,
            close: 100.0,
        },
    ];
    let series = series::Series::new(&bars, &[], &[]);
    let mut eval = btc::BtcEval::new(&series);
    assert_eq!(eval.at(T0)[0], (None, 0.0));
    // Once the coarse item expires, the remaining fine bar can answer again.
    assert_eq!(eval.at(T0 + 30_000)[0], (Some(0.0), 0.5));
}

/// Reusing the current price for older average steps changes the signed hourly delta.
#[test]
fn btc_average_steps_each_on_its_own_price() {
    let first_avg = btc::btc_avg_step(100.0, 110.0);
    let second_avg = btc::btc_avg_step(first_avg, 90.0);
    assert!(close(first_avg, 100.1));
    assert!(close(second_avg, 99.999));
    assert!(!close(second_avg, 99.801));
    let start = T0.div_euclid(CANDLE_MS) * CANDLE_MS;
    let bars = flat_bars(start - 60 * MINUTE_MS, start, 100.0);
    let ticks = [
        tick(start + 29_999, 110.0),
        tick(start + 30_001, 110.0),
        tick(start + 60_001, 90.0),
        tick(start + 60_002, 90.0),
        tick(start + 200_000, 90.0),
    ];
    let series = series::Series::new(&bars, &ticks, &[]);
    let mut eval = btc::BtcEval::new(&series);
    eval.at(start);
    // Item ends are exclusive; an update ending exactly 30 seconds after the seed cannot step.
    let exact = eval.at(start + 30_000)[2].0.unwrap();
    assert!(close(exact, 10.0), "{exact}");
    let first = eval.at(start + 30_002)[2].0.unwrap();
    assert!(close(first, (110.0 - 100.1) / 100.1 * 100.0), "{first}");
    let strict = eval.at(start + 60_002)[2].0.unwrap();
    assert!(close(strict, (90.0 - 100.1) / 100.1 * 100.0), "{strict}");
    let second = eval.at(start + 60_003)[2].0.unwrap();
    assert!(close(second, (90.0 - 99.999) / 99.999 * 100.0), "{second}");
    let gap = eval.at(start + 200_000)[2].0.unwrap();
    assert!(
        close(gap, second),
        "no update means no caught-up steps: {gap}"
    );
    let resumed = eval.at(start + 200_001)[2].0.unwrap();
    assert!(
        close(resumed, (90.0 - 99.89901) / 99.89901 * 100.0),
        "{resumed}"
    );
    let reseeded = eval.at(start + CANDLE_MS)[2].0.unwrap();
    assert!(close(reseeded, -10.0), "{reseeded}");
}

/// A five-minute low cannot be attributed to the last minute; the snapshot must survive.
#[test]
fn btc1m_on_five_minute_bars_is_unanswered() {
    let btc: Vec<Bar> = (T0 - 60 * MINUTE_MS..T0)
        .step_by(CANDLE_MS as usize)
        .map(|from_ms| Bar {
            from_ms,
            to_ms: from_ms + CANDLE_MS,
            open: 100.0,
            high: 100.0,
            low: 90.0,
            close: 100.0,
        })
        .collect();
    let inputs = TrackInputs {
        coin_bars: &[],
        ticks: &[tick(T0 - 1_000, 100.0)],
        covered: &[(T0 - 5_000, T0 + 5_000)],
        btc_bars: &btc,
        eval: (T0, T0 + 5_000),
        anchor: None,
    };
    let track = DeltaTrack::build(inputs).unwrap();
    assert_eq!(track.value(T0, DeltaField::Btc1m), None);
    assert!(close(
        track.value(T0, DeltaField::Btc5m).unwrap(),
        100.0 / 9.0
    ));
    let snapshot = Deltas {
        d1m: 1.0,
        btc1m: 0.7,
        btc5m: 0.8,
        ..Deltas::default()
    };
    let anchored = DeltaTrack::build(TrackInputs {
        anchor: Some((T0, &snapshot)),
        ..inputs
    })
    .unwrap();
    assert_eq!(anchored.stamp().coverage[DeltaField::Btc1m.index()], 0.0);
    assert_eq!(anchored.apply(T0, &snapshot).btc1m, 0.7);
    // A minute-sized latest bar does not make an older overlapping five-minute bar precise.
    let mixed = [
        Bar {
            from_ms: T0 - CANDLE_MS - 30_000,
            to_ms: T0 - 30_000,
            open: 100.0,
            high: 100.0,
            low: 90.0,
            close: 100.0,
        },
        Bar {
            from_ms: T0 - 30_000,
            to_ms: T0,
            open: 100.0,
            high: 100.0,
            low: 100.0,
            close: 100.0,
        },
    ];
    let series = series::Series::new(&mixed, &[], &[]);
    let point = btc::BtcEval::new(&series).at(T0);
    assert_eq!(point[0], (None, 0.0));
    assert!(point[1].0.is_some());
}

/// Using the recovered price instead of the hour's low would erase an earlier dump.
#[test]
fn dump1h_holds_the_hours_minimum() {
    let mut bars = flat_bars(T0 - 2 * 60 * MINUTE_MS, T0, 100.0);
    bars.iter_mut()
        .find(|b| b.from_ms == T0 - 20 * MINUTE_MS)
        .unwrap()
        .low = 90.0;
    let track = raw(
        &bars,
        &[tick(T0 + 1_000, 100.0)],
        (T0, T0 + 5_000),
        (T0, T0 + 5_000),
    );
    assert!(close(track.value(T0, DeltaField::Dump1h).unwrap(), 10.0));
}

#[test]
fn pump_and_dump_run_off_the_price_an_hour_ago() {
    // An hour ago the price was 100; within the hour it touched 110 and 95.
    let mut bars = flat_bars(T0 - 2 * 60 * MINUTE_MS, T0, 100.0);
    for b in &mut bars {
        if b.from_ms == T0 - 30 * MINUTE_MS {
            b.high = 110.0;
        }
        if b.from_ms == T0 - 20 * MINUTE_MS {
            b.low = 95.0;
        }
    }
    let ticks = [tick(T0 + 1_000, 100.0)];
    let track = raw(&bars, &ticks, (T0, T0 + 60_000), (T0, T0 + 60_000));
    assert!(close(track.value(T0, DeltaField::Pump1h).unwrap(), 10.0));
    assert!(close(track.value(T0, DeltaField::Dump1h).unwrap(), 5.0));
}

/// Reading the coin price, or stepping earlier updates at the last BTC close, changes BTC1h.
#[test]
fn btc_reads_its_own_market() {
    // BTC flat at 50 000 for four hours, then its last five minutes range 50 000 … 50 500.
    let mut btc = flat_bars(T0 - 4 * 60 * MINUTE_MS, T0, 50_000.0);
    let last = btc.len() - 1;
    btc[last].high = 50_500.0;
    btc[last].close = 50_500.0;
    let ticks = [tick(T0 + 1_000, 1.0)];
    let track = DeltaTrack::build(TrackInputs {
        coin_bars: &flat_bars(T0 - LOOKBACK_MS - CANDLE_MS, T0, 1.0),
        ticks: &ticks,
        covered: &[(T0, T0 + 60_000)],
        btc_bars: &btc,
        eval: (T0, T0 + 60_000),
        anchor: None,
    })
    .unwrap();
    assert!(close(track.value(T0, DeltaField::Btc1m).unwrap(), 1.0));
    assert!(close(track.value(T0, DeltaField::Btc5m).unwrap(), 1.0));
    // The seed at -200 s is 50 000. Updates at -120 s and -60 s stay flat; the update at
    // T0 is the first at 50 500, so exactly one step moves the average to 50 005.
    assert_eq!(T0.rem_euclid(CANDLE_MS), 200_000);
    let average = 50_005.0;
    let expected = (50_500.0 - average) / average * 100.0;
    assert!(close(track.value(T0, DeltaField::Btc1h).unwrap(), expected));
    // Without BTC's bars the fields keep the snapshot.
    let snapshot = Deltas {
        btc5m: 0.3,
        ..Deltas::default()
    };
    let bare = raw(&day_of_bars(), &ticks, (T0, T0 + 60_000), (T0, T0 + 60_000));
    assert!(close(bare.apply(T0, &snapshot).btc5m, 0.3));
}

#[test]
fn a_btc_field_the_report_left_at_zero_keeps_the_snapshot() {
    // A replica without the BTC columns, or a core without BTC's prices, files zeros: the track
    // must not put a live BTC field onto them. BTC moves 2 % after the stamp, so a live btc5m
    // would read it.
    let mut btc = flat_bars(T0 - 4 * 60 * MINUTE_MS, T0 + MINUTE_MS, 50_000.0);
    let last = btc.len() - 1;
    btc[last].high = 51_000.0;
    let snapshot = Deltas {
        d1h: 1.0,
        btc5m: 0.0,
        btc1h: 0.2,
        ..Deltas::default()
    };
    let track = DeltaTrack::build(TrackInputs {
        coin_bars: &day_of_bars(),
        ticks: &[tick(T0 + 1_000, 100.0)],
        covered: &[(T0, T0 + 60_000)],
        btc_bars: &btc,
        eval: (T0, T0 + 60_000),
        anchor: Some((T0 + 5_000, &snapshot)),
    })
    .unwrap();
    assert!(!track.is_live(DeltaField::Btc5m));
    assert!(track.is_live(DeltaField::Btc1h));
    // At +60 s the 2 % bar is in the window: the snapshot's 0 holds, not the move.
    assert!(close(track.apply(T0 + 60_000, &snapshot).btc5m, 0.0));
}

#[test]
fn the_snapshot_is_stamped_at_the_buy_for_moonshot_and_at_the_creation_otherwise() {
    let mut deal = crate::db::tuner::ticks::tests::deal();
    deal.buy_set_ms = Some(deal.buy_ms - 60_000);
    deal.kind = KIND_MOONSHOT.into();
    assert_eq!(snapshot_ms(&deal), Some(deal.buy_ms));
    deal.kind = "MoonHook".into();
    assert_eq!(snapshot_ms(&deal), Some(deal.buy_ms - 60_000));
}

#[test]
fn a_deal_reads_its_track_and_falls_back_to_the_snapshot() {
    let mut deal = crate::db::tuner::ticks::tests::deal();
    deal.deltas.d1m = 3.0;
    assert!(close(deal.deltas_at(deal.buy_ms).d1m, 3.0));
    let ticks = [tick(T0 + 1_000, 100.0), tick(T0 + 2_000, 102.0)];
    let track = raw(&day_of_bars(), &ticks, (T0, T0 + 60_000), (T0, T0 + 60_000));
    deal.delta_track = Some(Arc::new(track));
    assert!(close(deal.deltas_at(T0 + 5_000).d1m, 2.0));
    assert!(close(deal.deltas_at(T0 - 60_000).d1m, 3.0));
}

#[test]
fn the_summary_counts_live_fields_coverage_and_the_stamp_errors() {
    let snapshot = Deltas {
        d1m: 1.0,
        d1h: 1.0,
        ..Deltas::default()
    };
    let build = |bars: &[Bar]| {
        DeltaTrack::build(TrackInputs {
            coin_bars: bars,
            ticks: &[tick(T0 + 1_000, 100.0)],
            covered: &[(T0, T0 + 60_000)],
            btc_bars: &[],
            eval: (T0, T0 + 60_000),
            anchor: Some((T0 + 5_000, &snapshot)),
        })
        .unwrap()
    };
    let full = build(&day_of_bars());
    let short = build(&flat_bars(T0 - 30 * MINUTE_MS, T0, 100.0));
    let quality = summarize([&full, &short]);
    assert_eq!(quality.tracks, 2);
    let d1h = quality.fields[DeltaField::D1h.index()];
    assert_eq!((d1h.live, d1h.checked), (2, 2));
    // Both evaluated 0 against the report's 1: an error of 1 pp, not within 0.1.
    assert_eq!(d1h.reproduced, 0);
    assert_eq!(d1h.error_median, Some(1.0));
    // A field the report left at zero is not live anywhere.
    assert_eq!(quality.fields[DeltaField::D15m.index()].live, 0);
}
