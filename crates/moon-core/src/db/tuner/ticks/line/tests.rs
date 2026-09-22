//! The sell line's rules on synthetic tapes: one rule at a time, then the mirror.

use super::*;
use crate::db::tuner::ticks::exit::{ExitModel, archived_pre_spike_ask};
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
        core_name: String::new(),
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
        profit: None,
        deltas: Deltas::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: None,
        hook_depth_pct: None,
        hook_stated_take_pct: None,
        step_lag_ms: 0.0,
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

// ---- the take off the archive ----------------------------------------------------------------

/// `MShotSellAtLastPrice` reads the book's ask, which the tape has not; the archive's first
/// Exit point gives it back with the trade's own adjustment divided out, and a caller that
/// recovered it gets a take the tape alone would have put 0.5 % lower.
#[test]
fn the_archived_ask_sets_the_take_where_the_core_placed_it() {
    let p = ExitParams {
        sell_price_pct: 1.5,
        sell_at_last_price: true,
        sell_price_adjust_pct: 0.2,
        ..params()
    };
    // GSTOCKBSC, 2026-09-21: the take as placed, 0.031603, is the ask 0.0316663 less 0.2 %.
    let ask = archived_pre_spike_ask(Some(&[(0, 0.031603), (1_266, 0.030910)]), &p, false)
        .expect("the rule was on");
    assert!((ask - 0.031603 / 0.998).abs() < 1e-12);
    let mut d = deal(false);
    d.buy_price = 0.029292;
    d.pre_spike_ask = Some(ask);
    let f = Fill {
        t_ms: 0,
        price: 0.029292,
    };
    // The tape's own pre-spike print sits 0.5 % under the ask.
    let ticks = tape(&[(-5_000, 0.031506), (1_000, 0.0300)]);
    let take = ExitModel::new(&p).take_level(&d, &ticks, f);
    assert!((take - 0.031603).abs() < 1e-9, "{take}");
    d.pre_spike_ask = None;
    let from_tape = ExitModel::new(&p).take_level(&d, &ticks, f);
    assert!((from_tape - 0.031506 * 0.998).abs() < 1e-7, "{from_tape}");
    // With the rule off the archive says nothing about the ask.
    let off = ExitParams {
        sell_at_last_price: false,
        ..p.clone()
    };
    assert_eq!(
        archived_pre_spike_ask(Some(&[(0, 0.031603)]), &off, false),
        None
    );
    assert_eq!(archived_pre_spike_ask(None, &p, false), None);
    // A short's take is the ask adjusted UP toward the entry: divide the other way.
    let short_ask = archived_pre_spike_ask(Some(&[(0, 0.031603)]), &p, true).expect("on");
    assert!((short_ask - 0.031603 / 1.002).abs() < 1e-12);
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
fn the_placed_level_is_rounded_to_the_step() {
    // ARX, 2026-09-21, step 0.0001: take 0.197802 goes to the book as 0.1978; the 20 %
    // relative steps chain off the placed prices (0.1978 → 0.19714 → 0.1971 → 0.19658), and
    // the floor at +1 % (0.196445) is placed at 0.1964 — which is why the print AT 0.1964
    // sells.
    let p = ExitParams {
        price_down_timer_s: 3.0,
        price_down_pct: 20.0,
        price_down_delay_s: 10.0,
        price_down_relative: true,
        price_down_allowed_drop_pct: 1.0,
        ..params()
    };
    let mut d = deal(false);
    d.buy_price = 0.1945;
    d.tick = Some(0.0001);
    let fill = Fill {
        t_ms: 0,
        price: 0.1945,
    };
    let ticks = tape(&[
        (1_000, 0.1950),
        (4_000, 0.1950),
        (14_000, 0.1950),
        (24_000, 0.1950),
        (34_000, 0.1950),
        (40_000, 0.1964),
    ]);
    let w = walk(&d, &ticks, fill, 0.197802, &p);
    let levels: Vec<f64> = w.points.iter().map(|pt| pt.price).collect();
    let on_grid = |v: f64| (v / 0.0001).round() * 0.0001;
    assert_eq!(levels.len(), 4, "{levels:?}");
    for (level, want) in levels.iter().zip([0.1978, 0.1971, 0.1966, 0.1964]) {
        assert!((level - want).abs() < 1e-9, "{levels:?}");
        assert!(
            (level - on_grid(*level)).abs() < 1e-9,
            "off the grid: {level}"
        );
    }
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Line, 40_000));
    assert!(
        (w.exit.price - 0.1964).abs() < 1e-9,
        "sold at the placed level"
    );
}

#[test]
fn without_a_step_the_placed_level_is_the_exact_one() {
    let p = ExitParams {
        price_down_timer_s: 3.0,
        price_down_pct: 20.0,
        price_down_delay_s: 10.0,
        price_down_relative: true,
        price_down_allowed_drop_pct: 1.0,
        ..params()
    };
    let mut d = deal(false);
    d.buy_price = 0.1945;
    let fill = Fill {
        t_ms: 0,
        price: 0.1945,
    };
    let ticks = tape(&[(1_000, 0.1950), (40_000, 0.1964)]);
    let w = walk(&d, &ticks, fill, 0.197802, &p);
    assert_eq!(
        w.exit.kind,
        ExitKind::OpenAtWindowEnd,
        "0.196445 is above the tape's high"
    );
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

fn sold(t_ms: i64, price: f64) -> Tick {
    Tick {
        side: TickSide::Sell,
        ..tick(t_ms, price)
    }
}

/// The book-watching stop (`FastStopLoss` off) reads the BID through the prints that hit it —
/// taker sells — on its own sample clock, not every print through the level.
#[test]
fn the_book_stop_fires_on_a_sample_of_the_bid_not_on_a_print() {
    let book = ExitParams {
        stop_loss_pct: -1.0,
        fast_stop_loss: false,
        ..params()
    };
    // A taker BUY through the level says nothing about the BID; the taker sell at 98.8 does,
    // and the next sample after it — 4 s — fires, at the proxy's price.
    let ticks = vec![
        tick(1_000, 98.5),
        sold(1_500, 99.5),
        sold(2_500, 98.8),
        tick(5_000, 100.0),
    ];
    let w = walk(&deal(false), &ticks, fill(), 101.0, &book);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 4_000));
    assert!((w.exit.price - 98.8).abs() < 1e-4);
    // The fast stop takes the first print through the level, whichever side it hit.
    let fast = ExitParams {
        fast_stop_loss: true,
        ..book.clone()
    };
    let w = walk(&deal(false), &ticks, fill(), 101.0, &fast);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 1_000));
}

/// A sample due exactly at the tape's last print reads that print — the loop only reaches the
/// samples before a print, so the tape's end is where it must not be forgotten.
#[test]
fn the_book_stop_takes_the_sample_at_the_last_print() {
    let book = ExitParams {
        stop_loss_pct: -1.0,
        fast_stop_loss: false,
        ..params()
    };
    let w = walk(&deal(false), &[sold(2_000, 98.8)], fill(), 101.0, &book);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 2_000));
    // A sample the tape ends before is not taken: nothing is known past the last print.
    let w = walk(&deal(false), &[sold(1_500, 98.8)], fill(), 101.0, &book);
    assert_eq!(w.exit.kind, ExitKind::OpenAtWindowEnd);
}

/// `StopLossEMA` averages the samples, so a BID just past the level fires only once the
/// average is past it too.
#[test]
fn the_stop_ema_waits_for_the_average() {
    let ticks = vec![sold(1_500, 99.5), sold(2_500, 98.9), sold(9_000, 98.9)];
    let plain = ExitParams {
        stop_loss_pct: -1.0,
        fast_stop_loss: false,
        ..params()
    };
    let w = walk(&deal(false), &ticks, fill(), 101.0, &plain);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 4_000));
    // Samples 99.5, 98.9, 98.9, 98.9 at 2, 4, 6, 8 s: the EMA over 3 (α = 0.5) reads 99.5,
    // 99.2, 99.05, 98.975 — past 99 at the fourth.
    let smoothed = ExitParams {
        stop_loss_ema: 3.0,
        ..plain
    };
    let w = walk(&deal(false), &ticks, fill(), 101.0, &smoothed);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 8_000));
}

/// A book stop's sale is a panic sell walked through the book: the verdict holds the model's
/// stop level against the one the core printed, and the moment against the close — never the
/// sale price. A stop the core fired and the model never did is a miss.
#[test]
fn verify_judges_a_book_stop_by_its_level_and_moment() {
    let book = ExitParams {
        stop_loss_pct: -1.0,
        fast_stop_loss: false,
        ..params()
    };
    let mut d = deal(false);
    d.sell_reason = "StopLoss AutoActivated on price drop: BID = 98.800 ASK: 98.900 \
                     (strategy <S>); StopLoss fixed: 99.000 AllowedDrop: BUY -15.0%"
        .into();
    d.sell_price = 97.0;
    d.close_ms = 4_050;
    let ticks = vec![sold(1_500, 99.5), sold(2_500, 98.8), tick(5_000, 100.0)];
    let v = verify(&d, &ticks, &EntryParams::Fact, &book, None, None);
    assert_eq!(v.exit_kind, Some(ExitKind::Stop));
    assert_eq!(v.exit, Some(true), "{v:?}");
    assert!(v.exit_dev_pct.is_some_and(|dev| dev.abs() < 1e-9));
    // The stored reason cut inside the level: still the book stop, judged by its moment.
    let full = d.sell_reason.clone();
    d.sell_reason = full[..full.find("99.000").expect("level") + 3].to_string();
    let v = verify(&d, &ticks, &EntryParams::Fact, &book, None, None);
    assert_eq!(v.exit, Some(true), "{v:?}");
    d.close_ms = 9_000;
    let v = verify(&d, &ticks, &EntryParams::Fact, &book, None, None);
    assert_eq!(v.exit, Some(false), "five seconds off the moment, {v:?}");
    d.close_ms = 4_050;
    d.sell_reason = full;
    // The core's level elsewhere: not this stop.
    d.sell_reason = d.sell_reason.replace("99.000", "98.000");
    let v = verify(&d, &ticks, &EntryParams::Fact, &book, None, None);
    assert_eq!(v.exit, Some(false), "{v:?}");
    // A tape whose BID never reaches the level: the core stopped, the model did not.
    let calm = vec![sold(1_500, 99.5), tick(5_000, 100.0)];
    let v = verify(&d, &calm, &EntryParams::Fact, &book, None, None);
    assert_eq!(v.exit, Some(false), "{v:?}");
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

// ---- the core's sell price across a step --------------------------------------------------------

/// Relative PriceDown on the grid, flat tape under the line: `(t, level)` of every point.
fn grid_walk(take: f64, step_lag_ms: f64, ticks: &[Tick]) -> Vec<(i64, f64)> {
    let p = ExitParams {
        price_down_timer_s: 1.0,
        price_down_pct: 10.0,
        price_down_delay_s: 1.0,
        price_down_relative: true,
        price_down_allowed_drop_pct: 0.1,
        ..params()
    };
    let mut d = deal(false);
    d.buy_price = 1_000.0;
    d.tick = Some(1.0);
    d.step_lag_ms = step_lag_ms;
    let fill = Fill {
        t_ms: 0,
        price: 1_000.0,
    };
    walk(&d, ticks, fill, take, &p)
        .points
        .iter()
        .map(|pt| (pt.t_ms, pt.price))
        .collect()
}

/// A step that moved the order chains the next one off the ORDER's price: 1096 → 1086.4, placed
/// 1086; then 1086 → 1077.4, placed 1077 — off the exact 1086.4 it would round to 1078. INDEX
/// 2026-09-22 (Gate, 10 % relative): all twelve archived levels land only this way.
#[test]
fn a_moved_order_chains_off_its_placed_price() {
    let ticks = tape(&[(500, 1_000.0), (1_500, 1_000.0), (2_500, 1_000.0)]);
    assert_eq!(
        grid_walk(1_096.0, 0.0, &ticks),
        vec![(0, 1_096.0), (1_000, 1_086.0), (2_000, 1_077.0)]
    );
}

/// A step that rounds back onto the order sends nothing and carries its exact value into the
/// next: 1004 → 1003.6 (still 1004), → 1003.24 (1003), → 1002.7 (still 1003), → 1002.43 (1002).
/// Off the placed price alone the line would never leave 1004; off the exact chain the second
/// move would come a step late. FATCOIN 2026-09-22 climbs one tick every other step this way.
#[test]
fn a_step_rounding_back_onto_the_order_carries_its_exact_value() {
    let ticks = tape(&[
        (500, 1_000.0),
        (1_500, 1_000.0),
        (2_500, 1_000.0),
        (3_500, 1_000.0),
        (4_500, 1_000.0),
    ]);
    assert_eq!(
        grid_walk(1_004.0, 0.0, &ticks),
        vec![(0, 1_004.0), (2_000, 1_003.0), (4_000, 1_002.0)]
    );
}

/// The core's own replace lag spaces the steps: the first is timed off the take by
/// `PriceDownTimer`, the next one after a step that moved the order off that step plus the delay
/// plus the lag — here every step moves the order.
#[test]
fn the_core_step_lag_spaces_the_price_down_steps() {
    let ticks = tape(&[
        (500, 1_000.0),
        (1_500, 1_000.0),
        (2_500, 1_000.0),
        (3_500, 1_000.0),
    ]);
    let times: Vec<i64> = grid_walk(1_096.0, 50.0, &ticks)
        .iter()
        .map(|&(t, _)| t)
        .collect();
    assert_eq!(times, vec![0, 1_000, 2_050, 3_100]);
}

/// A step rounding kept in place sent nothing, so the next one waits for no round trip: the
/// lag follows only the steps that moved the order (1004 stays at 1 s, 1003 at 2 s, 1003 stays
/// at 3.05 s, 1002 at 4.05 s).
#[test]
fn a_step_kept_in_place_adds_no_lag() {
    let ticks = tape(&[
        (500, 1_000.0),
        (1_500, 1_000.0),
        (2_500, 1_000.0),
        (3_500, 1_000.0),
        (4_500, 1_000.0),
    ]);
    assert_eq!(
        grid_walk(1_004.0, 50.0, &ticks),
        vec![(0, 1_004.0), (2_000, 1_003.0), (4_050, 1_002.0)]
    );
}

// ---- PumpMove ------------------------------------------------------------------------------

/// `PumpMoveTimer` after the take, once: to `PumpMovePersent` of the peak-to-buy distance short
/// of the pump's peak — which printed before the buy. Peak 110, buy 100, 1 %: 109.9.
#[test]
fn the_pump_move_goes_once_to_the_peak_less_its_share() {
    let p = ExitParams {
        pump_move_timer_s: 2.0,
        pump_move_pct: 1.0,
        ..params()
    };
    let mut d = deal(false);
    d.kind = "PumpsDetection".into();
    d.tick = Some(0.01);
    let ticks = tape(&[
        (-3_000, 104.0),
        (-50, 110.0),
        (500, 101.0),
        (1_500, 102.0),
        (2_600, 101.5),
        (6_000, 101.0),
    ]);
    let w = walk(&d, &ticks, fill(), 104.0, &p);
    let points: Vec<(i64, f64)> = w.points.iter().map(|pt| (pt.t_ms, pt.price)).collect();
    assert_eq!(points.len(), 2, "{points:?}");
    assert_eq!(points[1].0, 2_000 + PUMP_MOVE_LAG_MS);
    assert!((points[1].1 - 109.9).abs() < 1e-9, "{points:?}");
}

// ---- the verdict's clock ---------------------------------------------------------------------

/// PriceDown 50 % relative every second, 100 ms to the book: the take and its steps.
fn clock_params(take_pct: f64) -> ExitParams {
    ExitParams {
        sell_price_pct: take_pct,
        price_down_timer_s: 1.0,
        price_down_pct: 50.0,
        price_down_delay_s: 1.0,
        price_down_allowed_drop_pct: 0.1,
        latency_ms: 100.0,
        ..params()
    }
}

/// The core stepped onto 100.5 at 900 ms and sold there 100 ms before the close — the archive
/// files the step and the fill as one point. The model's step to 100.5 is stamped 1 100 ms: late
/// by 200 ms, inside the point tolerance. On the model's own clock the fill found the take still
/// standing and the verdict had nothing to say; on the archive's the line stood at 100.5.
#[test]
fn the_level_at_the_fill_is_read_on_the_archive_clock() {
    let mut d = deal(false);
    d.sell_price = 100.5;
    d.close_ms = 1_000;
    let ticks = tape(&[(0, 100.0), (500, 100.0), (1_500, 100.0)]);
    let archived = [(0, 101.0), (900, 100.5)];
    let v = verify(
        &d,
        &ticks,
        &EntryParams::Fact,
        &clock_params(1.0),
        None,
        Some(&archived),
    );
    assert_eq!(v.exit_kind, Some(ExitKind::Line), "{v:?}");
    assert_eq!(v.exit, Some(true), "{v:?}");
}

/// A step the model took with no archived move to match: well before the fill it is a step the
/// core never took, and its level is the answer; inside the point tolerance of the fill it is
/// the model's timing, and the core's last level is.
#[test]
fn a_stray_step_counts_only_outside_the_point_tolerance_of_the_fill() {
    // Take 102; the model steps to 101 (1 100 ms), 100.5 (2 100 ms), 100.25 (3 100 ms). The
    // core stepped once, to 101, and filled a touch above it.
    let mut d = deal(false);
    d.sell_price = 101.02;
    let ticks = tape(&[
        (0, 100.0),
        (500, 100.0),
        (1_500, 100.0),
        (2_500, 100.0),
        (3_500, 100.0),
    ]);
    let p = clock_params(2.0);
    // Filled at 3 450 ms: the model's 100.5 at 2 100 ms is more than a second before it.
    d.close_ms = 3_600;
    let late = [(0, 102.0), (1_000, 101.0), (3_450, 101.02)];
    let v = verify(&d, &ticks, &EntryParams::Fact, &p, None, Some(&late));
    assert_eq!(v.exit, Some(false), "{v:?}");
    // Filled at 2 500 ms: the model's 100.5 came 400 ms before it.
    d.close_ms = 2_600;
    let soon = [(0, 102.0), (1_000, 101.0), (2_500, 101.02)];
    let v = verify(&d, &ticks, &EntryParams::Fact, &p, None, Some(&soon));
    assert_eq!(v.exit, Some(true), "{v:?}");
}

/// The fact's sell timers run from the take the core placed — the archive's first point, less
/// `SellDelay` — when that is later than the report's buy stamp.
#[test]
fn the_fact_sell_starts_at_the_archived_take() {
    let d = deal(false);
    let p = params();
    let start =
        |points: Option<&[(i64, f64)]>, p: &ExitParams| verify::fact_sell_start(&d, p, points).t_ms;
    assert_eq!(start(Some(&[(2_000, 101.0), (3_000, 100.5)]), &p), 2_000);
    let delayed = ExitParams {
        sell_delay_ms: 500.0,
        ..params()
    };
    assert_eq!(start(Some(&[(2_000, 101.0)]), &delayed), 1_500);
    assert_eq!(start(Some(&[(-7, 101.0)]), &p), 0, "never before the buy");
    assert_eq!(start(None, &p), 0);
}
