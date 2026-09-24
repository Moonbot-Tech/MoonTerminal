//! The step-lag calibration on synthetic archived lines.

use super::*;
use crate::db::tuner::ticks::Deltas;

fn deal() -> Deal {
    Deal {
        report_uid: 1,
        core_uid: 21,
        core_name: String::new(),
        strategy_id: 42,
        kind: "MoonHook".into(),
        coin: "COOL".into(),
        buy_ms: 0,
        close_ms: 60_000,
        buy_price: 100.0,
        sell_price: 100.2,
        spent: 1_000.0,
        is_short: false,
        sell_reason: "Auto Price Down".into(),
        fact_pnl: 0.2,
        profit: None,
        deltas: Deltas::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: None,
        hook_depth_pct: None,
        hook_stated_take_pct: None,
        step_lag_ms: 0.0,
        stop_anchor: None,
        delta_track: None,
        bars: None,
        own_entry: None,
        buy_set_ms: None,
        corridor: None,
        entry_placed: None,
    }
}

/// PriceDown every second after a 1 s timer.
fn price_down() -> ExitParams {
    ExitParams {
        price_down_timer_s: 1.0,
        price_down_delay_s: 1.0,
        price_down_pct: 10.0,
        ..ExitParams::default()
    }
}

/// The lag of every step after the first, off the core's own clock: the first step is timed off
/// the take, not off a step before it, and does not count.
#[test]
fn samples_are_the_steps_past_their_delay() {
    let archived = [
        (0, 101.0),
        (1_130, 100.9),
        (2_170, 100.8),
        (3_220, 100.7),
        (4_250, 100.6),
    ];
    assert_eq!(
        step_lag_samples(&deal(), &price_down(), &archived),
        vec![40, 50, 30]
    );
}

/// A pair two delays apart had a step between them that rounding kept in place; a pair closer
/// than one delay has another rule's move between them. Neither is a lag.
#[test]
fn skipped_and_foreign_steps_are_not_samples() {
    let archived = [
        (0, 101.0),
        (1_100, 100.9),
        (2_140, 100.8),
        (4_190, 100.7),  // a kept-in-place step at ~3.1 s
        (4_400, 100.65), // another rule's move
        (5_450, 100.6),
    ];
    assert_eq!(
        step_lag_samples(&deal(), &price_down(), &archived),
        vec![40, 50]
    );
}

/// The archive's fill point is no step, even one that lands a delay and a lag after the last
/// step and on the better side of it, as a limit's fill does.
#[test]
fn the_fill_point_is_not_a_step() {
    let mut d = deal();
    d.sell_price = 100.85;
    let archived = [(0, 101.0), (1_100, 100.9), (2_140, 100.8), (3_170, 100.85)];
    assert_eq!(step_lag_samples(&d, &price_down(), &archived), vec![40]);
}

/// No PriceDown, no samples — whatever the line did.
#[test]
fn a_line_without_price_down_gives_nothing() {
    let archived = [(0, 101.0), (1_100, 100.9), (2_140, 100.8)];
    assert!(step_lag_samples(&deal(), &ExitParams::default(), &archived).is_empty());
}

/// A re-place is filed as the old level's end at the answer, the new level's start at the
/// request, and the new level again at the answer: the round trip is the answer less the
/// request. Any other shape — a plain step, a point that repeats — is no re-place.
#[test]
fn the_replace_round_trip_reads_the_request_and_the_answer() {
    // FATCOIN on GateF, 2026-09-23: requested at 23 940, answered at 23 972.
    let line = [
        (0, 0.00191),
        (23_972, 0.00191),
        (23_940, 0.00189),
        (23_972, 0.00189),
        (29_347, 0.00189),
        (29_332, 0.00185),
        (29_347, 0.00185),
    ];
    assert_eq!(replace_round_trip_samples(&line), vec![32, 15]);
    // A line of plain steps, each at its own moment, holds none.
    let steps = [(0, 101.0), (1_000, 100.9), (2_000, 100.8)];
    assert!(replace_round_trip_samples(&steps).is_empty());
}

/// The median, and nothing from too few samples to trust.
#[test]
fn the_core_lag_is_the_median_of_enough_samples() {
    assert_eq!(median_step_lag(&mut [60, 10, 40, 50, 30]), Some(40.0));
    assert_eq!(median_step_lag(&mut [60, 10, 40, 50, 30, 70]), Some(45.0));
    assert_eq!(median_step_lag(&mut [10, 40, 50, 30]), None);
}
