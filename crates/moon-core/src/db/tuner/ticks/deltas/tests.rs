use super::*;
use crate::feed::types::Side;

const T0: i64 = 1_790_000_000_000; // a multiple of STEP_MS

fn tick(t_ms: i64, price: f32) -> Tick {
    Tick {
        time_ms: t_ms as f64,
        price,
        qty: 1.0,
        side: Side::Buy,
    }
}

/// Minute bars at a flat price from `from` to `to`, with one bar's extremes overridden.
fn flat_bars(from: i64, to: i64, price: f64) -> Vec<Bar> {
    (from..to)
        .step_by(MINUTE_MS as usize)
        .map(|t| Bar {
            from_ms: t,
            to_ms: t + MINUTE_MS,
            high: price,
            low: price,
        })
        .collect()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn a_delta_is_the_range_of_what_printed_before_the_boundary() {
    // History a whole day back, flat at 100; the tape from T0 on.
    let bars = flat_bars(T0 - LOOKBACK_MS - CANDLE_MS, T0, 100.0);
    let ticks = [tick(T0 + 1_000, 100.0), tick(T0 + 7_000, 110.0)];
    let covered = [(T0, T0 + 60_000)];
    let track = DeltaTrack::build(&bars, &ticks, &covered, (T0, T0 + 60_000), None).unwrap();
    // The print at +7 s joins at the next boundary, +10 s — not in the bucket it printed in.
    let (at_5s, _) = track.at(T0 + 9_999).unwrap();
    assert!(close(at_5s.d1m, 0.0), "{at_5s:?}");
    let (at_10s, live) = track.at(T0 + 10_000).unwrap();
    assert!(close(at_10s.d1m, 10.0), "{at_10s:?}");
    assert!(
        close(at_10s.d24h, 10.0) && close(at_10s.d3h, 10.0),
        "{at_10s:?}"
    );
    assert_eq!(live, [true; 6]);
}

#[test]
fn a_candle_window_reaches_one_candle_past_its_name() {
    // A spike to 120 in a bar that ended 19 minutes before T0: inside d15m's reach (20 min), and
    // out of it once the reach has passed it.
    let mut bars = flat_bars(T0 - LOOKBACK_MS - CANDLE_MS, T0, 100.0);
    let spike_end = T0 - 19 * MINUTE_MS;
    for b in &mut bars {
        if b.to_ms == spike_end {
            b.high = 120.0;
        }
    }
    let ticks = [tick(T0 + 1_000, 100.0)];
    let covered = [(T0, T0 + 5 * MINUTE_MS)];
    let track = DeltaTrack::build(&bars, &ticks, &covered, (T0, T0 + 5 * MINUTE_MS), None).unwrap();
    let (now, _) = track.at(T0).unwrap();
    assert!(close(now.d15m, 20.0), "{now:?}");
    assert!(close(now.d5m, 0.0), "{now:?}");
    // One minute later the bar ended 20 minutes ago: out of the window.
    let (later, _) = track.at(T0 + MINUTE_MS).unwrap();
    assert!(close(later.d15m, 0.0), "{later:?}");
    // Still inside d1h, and the long ones never read below it.
    assert!(close(later.d1h, 20.0) && close(later.d3h, 20.0) && close(later.d24h, 20.0));
}

#[test]
fn a_window_the_history_does_not_reach_back_for_keeps_the_snapshot() {
    // Six hours of bars: d1m … d3h live, d24h not.
    let bars = flat_bars(T0 - 6 * 60 * MINUTE_MS, T0, 100.0);
    let ticks = [tick(T0 + 1_000, 105.0)];
    let covered = [(T0, T0 + 60_000)];
    let track = DeltaTrack::build(&bars, &ticks, &covered, (T0, T0 + 60_000), None).unwrap();
    let (_, live) = track.at(T0 + 5_000).unwrap();
    assert_eq!(live, [true, true, true, true, true, false]);
    let snapshot = Deltas {
        d24h: 42.0,
        btc1h: 0.3,
        ..Deltas::default()
    };
    let applied = track.apply(T0 + 5_000, &snapshot);
    assert!(close(applied.d1h, 5.0), "{applied:?}");
    assert!(close(applied.d24h, 42.0), "the snapshot: {applied:?}");
    assert!(close(applied.btc1h, 0.3), "never live: {applied:?}");
    // Outside the covered stretch, the snapshot whole.
    assert_eq!(track.apply(T0 + 10 * MINUTE_MS, &snapshot), snapshot);
    // A hole of more than a candle in the history is where it stops.
    let mut holed = flat_bars(T0 - 6 * 60 * MINUTE_MS, T0, 100.0);
    holed.retain(|b| !(T0 - 30 * MINUTE_MS..T0 - 20 * MINUTE_MS).contains(&b.from_ms));
    let track = DeltaTrack::build(&holed, &ticks, &covered, (T0, T0 + 60_000), None).unwrap();
    let (_, live) = track.at(T0 + 5_000).unwrap();
    assert_eq!(live, [true, true, true, false, false, false]);
}

#[test]
fn nothing_live_is_no_track() {
    let ticks = [tick(T0 + 1_000, 100.0)];
    assert!(
        DeltaTrack::build(&[], &ticks, &[(T0, T0 + 30_000)], (T0, T0 + 30_000), None).is_none()
    );
}

#[test]
fn the_anchor_puts_the_track_on_the_report_at_its_stamp() {
    let bars = flat_bars(T0 - LOOKBACK_MS - CANDLE_MS, T0, 100.0);
    let ticks = [tick(T0 + 1_000, 100.0), tick(T0 + 12_000, 104.0)];
    let covered = [(T0, T0 + 60_000)];
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
    let track = DeltaTrack::build(
        &bars,
        &ticks,
        &covered,
        (T0, T0 + 60_000),
        Some((T0 + 5_000, &snapshot)),
    )
    .unwrap();
    let at_stamp = track.apply(T0 + 5_000, &snapshot);
    assert_eq!(at_stamp, snapshot);
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
}

#[test]
fn an_anchor_the_track_does_not_reach_is_no_track() {
    let bars = flat_bars(T0 - LOOKBACK_MS - CANDLE_MS, T0, 100.0);
    let ticks = [tick(T0 + 1_000, 100.0)];
    let snapshot = Deltas {
        d1m: 1.0,
        ..Deltas::default()
    };
    // Stamped before the tape begins: nothing to put the track on the report with.
    let stamp = Some((T0 - 60_000, &snapshot));
    assert!(
        DeltaTrack::build(
            &bars,
            &ticks,
            &[(T0, T0 + 60_000)],
            (T0, T0 + 60_000),
            stamp
        )
        .is_none()
    );
    // And a deal the report never stamped gets none either.
    let mut deal = crate::db::tuner::ticks::tests::deal();
    deal.kind = "MoonHook".into();
    deal.buy_set_ms = None;
    assert_eq!(snapshot_ms(&deal), None);
}

#[test]
fn a_field_not_live_at_the_stamp_is_not_live_anywhere() {
    // Two stretches: the first reached back six hours, the second a whole day — d24h is live
    // only in the second, and the stamp in the first gives it no offset.
    let bars = flat_bars(T0 - 6 * 60 * MINUTE_MS, T0, 100.0);
    let ticks = [tick(T0 + 1_000, 100.0)];
    let snapshot = Deltas {
        d1m: 1.0,
        d5m: 1.0,
        d15m: 1.0,
        d1h: 1.0,
        d3h: 1.0,
        d24h: 7.0,
        ..Deltas::default()
    };
    let track = DeltaTrack::build(
        &bars,
        &ticks,
        &[(T0, T0 + 60_000)],
        (T0, T0 + 60_000),
        Some((T0 + 5_000, &snapshot)),
    )
    .unwrap();
    let (_, live) = track.at(T0 + 30_000).unwrap();
    assert!(!live[5], "{live:?}");
    assert!(close(track.apply(T0 + 30_000, &snapshot).d24h, 7.0));
}

#[test]
fn a_bar_under_the_tape_is_not_history() {
    // A bar overlapping the tape carries prints from after the moment it would be read at.
    let mut bars = flat_bars(T0 - LOOKBACK_MS - CANDLE_MS, T0, 100.0);
    bars.push(Bar {
        from_ms: T0,
        to_ms: T0 + MINUTE_MS,
        high: 150.0,
        low: 100.0,
    });
    let ticks = [tick(T0 + 1_000, 100.0)];
    let covered = [(T0, T0 + MINUTE_MS)];
    let track = DeltaTrack::build(&bars, &ticks, &covered, (T0, T0 + MINUTE_MS), None).unwrap();
    let (late, _) = track.at(T0 + 55_000).unwrap();
    assert!(close(late.d1h, 0.0), "{late:?}");
}

#[test]
fn the_track_is_evaluated_only_where_the_models_read_it() {
    let bars = flat_bars(T0 - LOOKBACK_MS - CANDLE_MS, T0, 100.0);
    let ticks = [tick(T0 + 1_000, 100.0)];
    let covered = [(T0, T0 + 60 * MINUTE_MS)];
    let track = DeltaTrack::build(
        &bars,
        &ticks,
        &covered,
        (T0 + MINUTE_MS, T0 + 2 * MINUTE_MS),
        None,
    )
    .unwrap();
    assert!(track.at(T0 + 30_000).is_none());
    assert!(track.at(T0 + MINUTE_MS).is_some());
    assert!(track.at(T0 + 3 * MINUTE_MS).is_none());
}

#[test]
fn the_snapshot_is_stamped_at_the_buy_for_moonshot_and_at_the_creation_otherwise() {
    let mut deal = crate::db::tuner::ticks::tests::deal();
    deal.buy_set_ms = Some(deal.buy_ms - 60_000);
    deal.kind = KIND_MOONSHOT.into();
    assert_eq!(snapshot_ms(&deal), Some(deal.buy_ms));
    deal.kind = "MoonHook".into();
    assert_eq!(snapshot_ms(&deal), Some(deal.buy_ms - 60_000));
    deal.buy_set_ms = None;
    assert_eq!(snapshot_ms(&deal), None);
}

#[test]
fn a_deal_reads_its_track_and_falls_back_to_the_snapshot() {
    let mut deal = crate::db::tuner::ticks::tests::deal();
    deal.deltas.d1m = 3.0;
    assert!(close(deal.deltas_at(deal.buy_ms).d1m, 3.0));
    let bars = flat_bars(T0 - LOOKBACK_MS - CANDLE_MS, T0, 100.0);
    let ticks = [tick(T0 + 1_000, 100.0), tick(T0 + 2_000, 102.0)];
    let track =
        DeltaTrack::build(&bars, &ticks, &[(T0, T0 + 60_000)], (T0, T0 + 60_000), None).unwrap();
    deal.delta_track = Some(Arc::new(track));
    assert!(close(deal.deltas_at(T0 + 5_000).d1m, 2.0));
    assert!(close(deal.deltas_at(T0 - 60_000).d1m, 3.0));
}
