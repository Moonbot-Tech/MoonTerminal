//! The Sell order section on synthetic tapes: the take, PriceDown, SellLevel.

use super::*;
use crate::db::tuner::ticks::ExitKind;
use crate::db::tuner::ticks::exit::line::walk;
use crate::db::tuner::ticks::exit::tests::{deal, fill, params, tape};

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
    // A short's take is the ask adjusted UP toward the entry, `ask / (1 − 0.2 %)`: the ask is
    // the take times 0.998.
    let short_ask = archived_pre_spike_ask(Some(&[(0, 0.031603)]), &p, true).expect("on");
    assert!((short_ask - 0.031603 * 0.998).abs() < 1e-12);
}

/// A short's take is divided off the fill, `fill / (1 + SellPrice/100)`, for every kind, and a
/// MoonShot's ask branch placed at `ask / (1 − adjust/100)` when it is the lower (the core
/// developer, 2026-09-23 and answer 9 of 2026-09-24).
#[test]
fn a_short_take_divides_off_the_fill() {
    let p = ExitParams {
        sell_price_pct: 1.0,
        ..params()
    };
    let take = ExitModel::new(&p).take_level(&deal(true), &[], fill());
    assert!((take - 100.0 / 1.01).abs() < 1e-9, "{take}");
    let lifted = ExitParams {
        sell_at_last_price: true,
        sell_price_adjust_pct: 1.0,
        ..p
    };
    let mut d = deal(true);
    d.pre_spike_ask = Some(97.0);
    let take = ExitModel::new(&lifted).take_level(&d, &[], fill());
    assert!((take - 97.0 / 0.99).abs() < 1e-9, "{take}");
    // A MoonHook short divides too — not the product's 99 — and so does its own take rule.
    let mut hook = deal(true);
    hook.kind = "MoonHook".into();
    let take = ExitModel::new(&p).take_level(&hook, &[], fill());
    assert!((take - 100.0 / 1.01).abs() < 1e-9, "{take}");
    hook.hook_depth_pct = Some(4.0);
    let rule = ExitParams {
        hook_sell_level_pct: 50.0,
        ..p.clone()
    };
    let take = ExitModel::new(&rule).take_level(&hook, &[], fill());
    assert!((take - 100.0 / 1.02).abs() < 1e-9, "{take}");
    // A long's take is the product, as ever.
    let take = ExitModel::new(&p).take_level(&deal(false), &[], fill());
    assert!((take - 101.0).abs() < 1e-9, "{take}");
}

/// A short's `PriceDownAllowedDrop` floor is divided off the buy — `buy / (1 + drop/100)` — as
/// the archived short lines stop on it (67 against 1 for the product, 2026-09-24); the step
/// without `Relative` stays a share of the price.
#[test]
fn a_short_price_down_floor_divides_off_the_buy() {
    let p = ExitParams {
        price_down_timer_s: 1.0,
        price_down_pct: 100.0,
        price_down_delay_s: 1.0,
        price_down_relative: true,
        price_down_allowed_drop_pct: 0.5,
        ..params()
    };
    // Take 99; a relative step of 100 % goes to the buy, the floor stops it at 100 / 1.005 —
    // not the product's 99.5.
    let ticks = tape(&[(1_500, 99.6), (2_500, 99.6)]);
    let w = walk(&deal(true), &ticks, fill(), 99.0, &p);
    assert!(
        (w.points[1].price - 100.0 / 1.005).abs() < 1e-9,
        "{:?}",
        w.points
    );
    assert_eq!(w.points.len(), 2, "no step past the floor: {:?}", w.points);
    let absolute = ExitParams {
        price_down_pct: 0.2,
        price_down_relative: false,
        ..p
    };
    let w = walk(&deal(true), &ticks, fill(), 99.0, &absolute);
    assert!((w.points[1].price - 99.2).abs() < 1e-9, "{:?}", w.points);
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

/// The look-back reads the market's bars where the tape does not reach — BROCCOLI714 24.09: an
/// hour's look-back read off a tape of minutes put the sell at the run-up's high — and only the
/// bars inside it that closed by the move: one still open holds prints from after it, one that
/// began before the look-back holds prints from before it.
#[test]
fn sell_level_reads_the_bars_before_the_tape() {
    let p = ExitParams {
        sell_level_delay_s: 2.0,
        sell_level_time_s: 3_600.0,
        sell_level_count: 1,
        sell_level_adjust_pct: 0.0,
        ..params()
    };
    let bar = |from_ms: i64, to_ms: i64, high: f64| Bar {
        from_ms,
        to_ms,
        open: 100.0,
        high,
        low: 99.0,
        close: 100.0,
    };
    let mut d = deal(false);
    d.bars = Some(
        vec![
            // Began before the look-back (2 000 − 3 600 000): not read.
            bar(-3_660_000, -3_540_000, 130.0),
            // Inside it and closed: the high the core saw.
            bar(-600_000, -540_000, 110.0),
            // Still open at the move: not read.
            bar(0, 60_000, 125.0),
        ]
        .into(),
    );
    let ticks = tape(&[(500, 103.0), (1_000, 100.5), (2_500, 100.5)]);
    let w = walk(&d, &ticks, fill(), 105.0, &p);
    assert!((w.points[1].price - 110.0).abs() < 1e-9, "{:?}", w.points);
    // Without the bars the tape's own high is all there is.
    let w = walk(&deal(false), &ticks, fill(), 105.0, &p);
    assert!((w.points[1].price - 103.0).abs() < 1e-9, "{:?}", w.points);
}

/// SellLevel runs in the timers' race: a move due before a PriceDown step goes first, and the step
/// chains off the level it left — not the other way round because both fell due by one print.
#[test]
fn sell_level_and_price_down_go_in_the_order_they_fall_due() {
    let p = ExitParams {
        price_down_timer_s: 3.0,
        price_down_pct: 50.0,
        price_down_delay_s: 10.0,
        price_down_relative: true,
        price_down_allowed_drop_pct: 0.0,
        sell_level_delay_s: 2.9,
        sell_level_time_s: 10.0,
        sell_level_count: 1,
        sell_level_adjust_pct: 0.0,
        ..params()
    };
    // Both are due by the print at 3 500: SellLevel at 2 900 from the take 105 to the high 104,
    // then PriceDown at 3 000 halfway from 104 to the buy — not PriceDown off 105 to 102.5 first.
    let ticks = tape(&[(500, 104.0), (3_500, 100.5)]);
    let w = walk(&deal(false), &ticks, fill(), 105.0, &p);
    let levels: Vec<(i64, f64)> = w.points.iter().map(|pt| (pt.t_ms, pt.price)).collect();
    assert_eq!(levels[1], (2_900, 104.0), "{levels:?}");
    assert!((levels[2].1 - 102.0).abs() < 1e-9, "{levels:?}");
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
