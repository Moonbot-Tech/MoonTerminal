//! The sell line's rules on synthetic tapes: one rule at a time, then the mirror.

use super::*;
use crate::db::tuner::ticks::exit::ExitModel;
use crate::db::tuner::ticks::{Deltas, EntryParams, verify};
use crate::feed::types::Side as TickSide;

fn tick(t_ms: i64, price: f64) -> Tick {
    Tick {
        time_ms: t_ms as f64,
        price: price as f32,
        qty: 1.0,
        side: TickSide::Buy,
    }
}

fn tape(points: &[(i64, f64)]) -> Vec<Tick> {
    points.iter().map(|&(t, p)| tick(t, p)).collect()
}

fn deal(short: bool) -> Deal {
    Deal {
        report_uid: 1,
        core_uid: 7,
        strategy_id: 42,
        kind: "MoonShot".into(),
        coin: "ACE".into(),
        buy_ms: 0,
        close_ms: 60_000,
        buy_price: 100.0,
        sell_price: 100.5,
        spent: 1_000.0,
        is_short: short,
        sell_reason: "Auto Price Down".into(),
        fact_pnl: 5.0,
        deltas: Deltas::default(),
        tick: None,
    }
}

fn fill() -> Fill {
    Fill {
        t_ms: 0,
        price: 100.0,
    }
}

/// A 1 % take, no latency, and the rule under test.
fn params() -> ExitParams {
    ExitParams {
        latency_ms: 0.0,
        ..ExitParams::default()
    }
}

// ---- PriceDown -------------------------------------------------------------------------------

#[test]
fn price_down_steps_the_line_toward_the_buy_on_the_timer() {
    // Take 101. Timer 1 s, then every 1 s, 50 % of the remaining distance (relative): 100.5
    // at t=1000, 100.25 at t=2000, floored at +0.1 % = 100.1.
    let p = ExitParams {
        price_down_timer_s: 1.0,
        price_down_pct: 50.0,
        price_down_delay_s: 1.0,
        price_down_relative: true,
        price_down_allowed_drop_pct: 0.1,
        ..params()
    };
    let ticks = tape(&[
        (500, 100.0),
        (1_500, 100.0),
        (2_500, 100.0),
        (3_500, 100.0),
        (4_500, 100.0),
        (5_000, 100.2),
    ]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    let levels: Vec<f64> = w.points.iter().map(|pt| pt.price).collect();
    assert!((levels[0] - 101.0).abs() < 1e-9);
    assert!((levels[1] - 100.5).abs() < 1e-9, "{levels:?}");
    assert!((levels[2] - 100.25).abs() < 1e-9, "{levels:?}");
    assert!((levels[3] - 100.125).abs() < 1e-9, "{levels:?}");
    assert!((levels[4] - 100.1).abs() < 1e-9, "the floor: {levels:?}");
    assert_eq!(levels.len(), 5, "no step past the floor");
    // The print at 100.2 at t=5000 crosses the line at 100.1.
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Line, 5_000));
    assert!((w.exit.price - 100.1).abs() < 1e-9);
}

#[test]
fn price_down_absolute_takes_a_share_of_the_price() {
    // SellPrice 1 %, pct 0.2 absolute: 101 -> 100.8 (of the buy price).
    let p = ExitParams {
        price_down_timer_s: 1.0,
        price_down_pct: 0.2,
        price_down_relative: false,
        price_down_allowed_drop_pct: 0.0,
        ..params()
    };
    let ticks = tape(&[(1_500, 100.0), (2_000, 100.9)]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    assert!((w.points[1].price - 100.8).abs() < 1e-9, "{:?}", w.points);
    assert_eq!(w.exit.kind, ExitKind::Line);
}

#[test]
fn a_zero_timer_never_starts_the_steps() {
    let p = ExitParams {
        price_down_timer_s: 0.0,
        price_down_pct: 50.0,
        ..params()
    };
    let ticks = tape(&[(5_000, 100.0), (60_000, 100.5)]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    assert_eq!(w.points.len(), 1);
    assert_eq!(w.exit.kind, ExitKind::OpenAtWindowEnd, "nothing closed it");
}

#[test]
fn a_zero_delay_steps_at_the_terminal_floor() {
    let p = ExitParams {
        price_down_timer_s: 1.0,
        price_down_pct: 50.0,
        price_down_delay_s: 0.0,
        price_down_allowed_drop_pct: -1.0,
        ..params()
    };
    let ticks = tape(&[(1_000, 100.0), (1_660, 100.0)]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    // t=1000, 1330, 1660: three steps by the second print.
    assert_eq!(w.points.len(), 4, "{:?}", w.points);
    assert_eq!(w.points[2].t_ms, 1_330);
}

// ---- SellLevel -------------------------------------------------------------------------------

#[test]
fn sell_level_moves_to_the_look_back_high_adjusted() {
    // Delay 2 s, look back 10 s, adjust -1 %: the high of the last 10 s at t=2000 is 103 ->
    // 101.97; once (count 1).
    let p = ExitParams {
        sell_level_delay_s: 2.0,
        sell_level_time_s: 10.0,
        sell_level_count: 1,
        sell_level_adjust_pct: -1.0,
        sell_level_allowed_drop_pct: 0.0,
        ..params()
    };
    let ticks = tape(&[(500, 103.0), (1_000, 100.5), (2_000, 100.5), (3_000, 102.0)]);
    let w = walk(&deal(false), &ticks, fill(), 105.0, &p);
    assert!((w.points[1].price - 101.97).abs() < 1e-9, "{:?}", w.points);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Line, 3_000));
}

#[test]
fn sell_level_relative_takes_a_share_of_the_distance_to_the_buy() {
    let p = ExitParams {
        sell_level_delay_s: 1.0,
        sell_level_time_s: 10.0,
        sell_level_count: 1,
        sell_level_adjust_pct: 50.0,
        sell_level_relative: true,
        ..params()
    };
    let ticks = tape(&[(500, 110.0), (1_000, 100.0)]);
    let w = walk(&deal(false), &ticks, fill(), 120.0, &p);
    assert!((w.points[1].price - 105.0).abs() < 1e-9, "{:?}", w.points);
}

// ---- SellShot ---------------------------------------------------------------------------------

#[test]
fn sell_shot_follows_the_market_inside_its_corridor() {
    // Distance 1 %, corridor 50 %: the line stays while 0.5-1.5 % off the reference. The
    // take at 101 is 1 % off 100; a rise to 100.8 leaves it 0.2 % off -> too close -> after
    // the raise wait (0) it moves away to 101.808.
    let p = ExitParams {
        ignore_sell_shot: false,
        sell_shot_distance_pct: 1.0,
        sell_shot_corridor_pct: 50.0,
        sell_shot_calc_interval_s: 0.1,
        sell_shot_allowed_up_pct: 10.0,
        sell_shot_allowed_down_pct: -1.0,
        ..params()
    };
    let ticks = tape(&[(500, 100.0), (1_000, 100.8), (1_500, 101.5)]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    assert!((w.points[1].price - 101.808).abs() < 1e-4, "{:?}", w.points);
    // 101.5 stays under the moved line, and the tape ends before the report's close.
    assert_eq!(w.exit.kind, ExitKind::OpenAtWindowEnd);
}

#[test]
fn sell_shot_is_capped_by_allowed_up() {
    let p = ExitParams {
        ignore_sell_shot: false,
        sell_shot_distance_pct: 1.0,
        sell_shot_corridor_pct: 50.0,
        sell_shot_calc_interval_s: 0.1,
        sell_shot_allowed_up_pct: 0.5,
        sell_shot_allowed_down_pct: -1.0,
        ..params()
    };
    let ticks = tape(&[(500, 100.0), (1_000, 100.8)]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    assert!((w.points[1].price - 100.5).abs() < 1e-4, "{:?}", w.points);
}

// ---- StopLoss ---------------------------------------------------------------------------------

#[test]
fn the_stop_fires_on_the_print_after_its_delay() {
    let p = ExitParams {
        stop_loss_pct: -1.0,
        stop_loss_delay_s: 2.0,
        ..params()
    };
    // A print through the stop inside the delay does not fire; one after it does, at the
    // print's price (a market exit).
    let ticks = tape(&[(1_000, 98.5), (3_000, 98.7)]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 3_000));
    assert!((w.exit.price - 98.7).abs() < 1e-4);
}

// ---- the mirror and the archive -------------------------------------------------------------

#[test]
fn a_short_mirrors_every_rule() {
    let p = ExitParams {
        price_down_timer_s: 1.0,
        price_down_pct: 50.0,
        price_down_delay_s: 1.0,
        price_down_allowed_drop_pct: 0.1,
        stop_loss_pct: -1.0,
        ..params()
    };
    // Take 99 (1 % below the buy at 100); one step -> 99.5 at t=1000; a print down to 99.4
    // before the next step crosses it.
    let ticks = tape(&[(1_500, 100.0), (1_900, 99.4)]);
    let w = walk(&deal(true), &ticks, fill(), 99.0, &p);
    assert!((w.points[1].price - 99.5).abs() < 1e-9, "{:?}", w.points);
    assert_eq!(w.exit.kind, ExitKind::Line);
    assert!((w.exit.price - 99.5).abs() < 1e-9);
    // The stop is ABOVE a short's buy.
    let ticks = tape(&[(500, 101.2)]);
    let w = walk(&deal(true), &ticks, fill(), 99.0, &p);
    assert_eq!(w.exit.kind, ExitKind::Stop);
}

#[test]
fn a_spike_through_the_old_level_fills_before_a_step_lands() {
    let p = ExitParams {
        price_down_timer_s: 1.0,
        price_down_pct: 50.0,
        latency_ms: 100.0,
        ..params()
    };
    // The step is decided at t=1000 and reaches the book at t=1100; a print at 101 at t=1050
    // fills the old take at 101, not the new line at 100.5.
    let ticks = tape(&[(1_000, 100.0), (1_050, 101.0)]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    assert_eq!(w.exit.kind, ExitKind::Take);
    assert!((w.exit.price - 101.0).abs() < 1e-9);
}

#[test]
fn verify_holds_the_line_against_the_archived_points() {
    let p = ExitParams {
        price_down_timer_s: 1.0,
        price_down_pct: 50.0,
        price_down_delay_s: 1.0,
        price_down_allowed_drop_pct: 0.1,
        ..params()
    };
    let mut d = deal(false);
    d.sell_price = 100.25;
    let ticks = tape(&[(0, 100.0), (1_500, 100.0), (2_500, 100.3)]);
    // The archive's points are what the core did: placed at 101, 100.5 at 1 s, 100.25 at 2 s.
    let archived = [(0, 101.0), (1_000, 100.5), (2_000, 100.25)];
    let v = verify(&d, &ticks, &EntryParams::Fact, &p, None, Some(&archived));
    assert_eq!(v.exit_kind, Some(ExitKind::Line));
    assert_eq!(v.exit, Some(true), "{v:?}");
    assert_eq!(v.line_points, Some((3, 3)));
    // A line the core moved differently - a point the model never re-placed at - is a miss
    // even when the exit price agrees.
    let other = [(0, 101.0), (1_000, 100.7), (2_000, 100.25)];
    let v = verify(&d, &ticks, &EntryParams::Fact, &p, None, Some(&other));
    assert_eq!(v.exit, Some(false));
    assert_eq!(v.line_points, Some((2, 3)));
    // The same walk through `ExitModel`.
    let w = ExitModel::new(&p).walk(&d, &ticks, fill());
    assert_eq!(w.points.len(), 3);
}
