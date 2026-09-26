//! The stop ladder on synthetic tapes: when a step is taken, where it moves the stop, and that it
//! stays.

use super::*;
use crate::db::tuner::ticks::ExitKind;
use crate::db::tuner::ticks::exit::ExitParams;
use crate::db::tuner::ticks::exit::line::walk;
use crate::db::tuner::ticks::exit::tests::{deal, fill, params};

/// A print on the bid's side of the book — a taker sell.
fn sell(t_ms: i64, price: f64) -> Tick {
    Tick {
        time_ms: t_ms as f64,
        price: price as f32,
        qty: 1.0,
        side: TickSide::Sell,
    }
}

const PERIOD: i64 = super::super::TICKER_PERIOD_MS;

fn step(after_s: f64, switch_pct: f64, level_pct: f64) -> StopStep {
    StopStep {
        after_s,
        switch_pct,
        level_pct,
    }
}

/// The step waits for its time, then for the bid past the switch price, on the ticker's
/// arrivals: a bid past it at one second does not move a five-second step before the first
/// arrival after five seconds.
#[test]
fn a_step_waits_for_its_time_and_the_bid() {
    let mut ladder = Ladder::new(
        true,
        100.0,
        0,
        0,
        0,
        0,
        PERIOD,
        &[Some(step(5.0, 0.5, 0.4))],
    )
    .expect("a step");
    ladder.see(&sell(1_000, 101.0));
    // Arrivals at 2 150 and 4 300: too early.
    assert_eq!(ladder.before(5_000), None);
    // The arrival at 6 450 takes it: the stop moves to 0.4 % over the buy.
    let level = ladder.before(7_000).expect("taken");
    assert!((level - 100.4).abs() < 1e-9, "{level}");
    // Taken once: the bid falling back does not move it again, nor back.
    ladder.see(&sell(7_500, 99.0));
    assert_eq!(ladder.before(20_000), None);
}

/// A bid short of the switch price takes nothing, however long it waits.
#[test]
fn a_bid_short_of_the_switch_price_takes_nothing() {
    let mut ladder = Ladder::new(
        true,
        100.0,
        0,
        0,
        0,
        0,
        PERIOD,
        &[Some(step(0.0, 1.0, 0.4))],
    )
    .expect("a step");
    ladder.see(&sell(100, 100.9));
    assert_eq!(ladder.before(60_000), None);
}

/// A short's switch price and level divide the buy, and its bid must be BELOW the switch price.
#[test]
fn a_short_step_divides_the_buy() {
    let mut ladder = Ladder::new(
        false,
        100.0,
        0,
        0,
        0,
        0,
        PERIOD,
        &[Some(step(0.0, 0.5, 0.4))],
    )
    .expect("a step");
    // 100 / 1.005 = 99.502: a bid of 99.6 is not past it, 99.4 is.
    ladder.see(&sell(100, 99.6));
    assert_eq!(ladder.before(3_000), None);
    ladder.see(&sell(3_000, 99.4));
    let level = ladder.before(5_000).expect("taken");
    assert!((level - 100.0 / 1.004).abs() < 1e-9, "{level}");
}

/// The third stop is its own step: taken after the second, it moves the stop again.
#[test]
fn the_third_step_moves_the_stop_again() {
    let steps = [Some(step(0.0, 0.5, 0.4)), Some(step(0.0, 2.0, 1.2))];
    let mut ladder = Ladder::new(true, 100.0, 0, 0, 0, 0, PERIOD, &steps).expect("steps");
    ladder.see(&sell(100, 100.8));
    let second = ladder.before(3_000).expect("second");
    assert!((second - 100.4).abs() < 1e-9);
    ladder.see(&sell(3_000, 102.5));
    let third = ladder.before(5_000).expect("third");
    assert!((third - 101.2).abs() < 1e-9);
}

/// In the walk: after the second stop is taken, a fall that the first stop would sit through
/// fires the stop at the second's level.
#[test]
fn the_walk_stops_at_the_second_level() {
    let p = ExitParams {
        stop_loss_pct: -5.0,
        fast_stop_loss: true,
        second_stop: Some(step(1.0, 0.5, 0.4)),
        ..params()
    };
    let ticks = [sell(500, 100.8), sell(4_000, 100.9), sell(6_000, 100.3)];
    let w = walk(&deal(false), &ticks, fill(), 105.0, &p);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 6_000), "{w:?}");
    assert!(
        w.stop_level.is_some_and(|l| (l - 100.4).abs() < 1e-9),
        "{w:?}"
    );
    // Without the ladder the first stop sits at 95 and nothing fires.
    let plain = ExitParams {
        second_stop: None,
        ..p
    };
    let w = walk(&deal(false), &ticks, fill(), 105.0, &plain);
    assert_ne!(w.exit.kind, ExitKind::Stop, "{w:?}");
}

/// No stop, no ladder: the fields hang on `UseStopLoss`.
#[test]
fn no_stop_no_ladder() {
    let p = ExitParams {
        stop_loss_pct: 0.0,
        fast_stop_loss: true,
        second_stop: Some(step(0.0, 0.5, 0.4)),
        ..params()
    };
    let ticks = [sell(500, 100.8), sell(4_000, 100.9), sell(6_000, 100.3)];
    let w = walk(&deal(false), &ticks, fill(), 105.0, &p);
    assert_ne!(w.exit.kind, ExitKind::Stop, "{w:?}");
    assert_eq!(w.stop_level, None);
}

/// `StopLossDelay` holds the ladder: a bid past the switch price inside the delay and back under
/// it by the delay's end takes nothing (the core's answer 2, 2026-09-24).
#[test]
fn the_delay_holds_the_ladder() {
    let mut ladder = Ladder::new(
        true,
        100.0,
        0,
        0,
        6_000,
        0,
        PERIOD,
        &[Some(step(1.0, 0.5, 0.4))],
    )
    .expect("a step");
    ladder.see(&sell(1_000, 101.0));
    assert_eq!(ladder.before(5_000), None, "inside the delay");
    ladder.see(&sell(5_000, 100.2));
    assert_eq!(
        ladder.before(20_000),
        None,
        "the bid came back before the first read"
    );
}

/// The time counts from the sell's placement, its seconds rounded: a step at 2 s off a sell placed
/// at 1 s is not taken before 3.5 s (the core's answer 3).
#[test]
fn the_time_counts_from_the_sell_rounded() {
    let mut ladder = Ladder::new(
        true,
        100.0,
        0,
        1_000,
        0,
        0,
        PERIOD,
        &[Some(step(2.0, 0.5, 0.4))],
    )
    .expect("a step");
    ladder.see(&sell(100, 101.0));
    // The arrival at 2 150 is before 3 500; the one at 4 300 is after.
    assert_eq!(ladder.before(3_000), None);
    assert!(ladder.before(5_000).is_some());
}
