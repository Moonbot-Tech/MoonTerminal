use super::*;
use crate::feed::types::Side;

const T0: i64 = 1_790_000_000_000; // a multiple of STEP_MS and of a minute

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
    // The six hours' range against the report's twelve: the error before the anchor.
    let range = (105.0 / 95.0 - 1.0) * 100.0;
    let error = stamp.error[DeltaField::D24h.index()].unwrap();
    assert!(close(error, 12.0 - range), "{error}");
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
    assert_eq!(track.stamp().error[DeltaField::D1m.index()], Some(1.0));
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

#[test]
fn d5s_is_the_move_over_the_last_bucket() {
    let ticks = [
        tick(T0 + 1_000, 100.0),
        tick(T0 + 6_000, 102.0),
        tick(T0 + 8_000, 101.0),
    ];
    let track = raw(&day_of_bars(), &ticks, (T0, T0 + 20_000), (T0, T0 + 20_000));
    // No previous boundary for the first point.
    assert!(track.value(T0, DeltaField::D5s).is_none());
    // +10 s: the last price before it (101) against the last before +5 s (100).
    assert!(close(
        track.value(T0 + 10_000, DeltaField::D5s).unwrap(),
        1.0
    ));
    // +15 s: nothing printed in the bucket.
    assert!(close(
        track.value(T0 + 15_000, DeltaField::D5s).unwrap(),
        0.0
    ));
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
    // One minute bar is two of the average's steps: it moved 1 − 0.99² of the way up.
    let average = 50_000.0 + 500.0 * (1.0 - 0.99f64.powi(2));
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
