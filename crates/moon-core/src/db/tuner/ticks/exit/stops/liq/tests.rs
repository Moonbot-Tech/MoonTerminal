//! The liquidation guards: where `StopAboveLiq` holds a stop, and where `DontSellBelowLiq` holds
//! a sale. Every expected price is worked out by hand from the buy of 100.

use super::*;
use crate::db::tuner::ticks::ExitKind;
use crate::db::tuner::ticks::exit::level_off_buy;
use crate::db::tuner::ticks::exit::line::walk;
use crate::db::tuner::ticks::exit::tests::{deal, fill, params, tape};

/// A trade at `leverage`, its strategy running `StopAboveLiq`.
fn guarded(short: bool, leverage: f64, above_pct: f64) -> (Deal, ExitParams) {
    let mut d = deal(short);
    d.leverage = Some(leverage);
    let mut p = params();
    p.stop_above_liq_pct = above_pct;
    (d, p)
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// A wide `StopAboveLiq` on a leveraged position lifts a deep stop to 0.2 % off the buy — the
/// level 41 of 41 live stops printed. Dropping the hold would put it past the buy.
#[test]
fn a_wide_guard_holds_the_stop_at_a_fifth_of_a_per_cent() {
    let (long, p) = guarded(false, 5.0, 50.0);
    let deep = level_off_buy(100.0, -6.5, true);
    assert!(near(LiqGuard::new(&p, &long, 100.0).stop(deep), 99.8));
    let (short, p) = guarded(true, 5.0, 50.0);
    let deep = level_off_buy(100.0, -6.5, false);
    assert!(near(LiqGuard::new(&p, &short, 100.0).stop(deep), 100.2));
}

/// A narrow guard follows the liquidation estimate: at a leverage of 20 a long liquidates near
/// 95, and 2 % over that is 96.9 — a stop at 95 moves up, one at 98 stays.
#[test]
fn a_narrow_guard_stands_over_the_liquidation_estimate() {
    let (long, p) = guarded(false, 20.0, 2.0);
    let guard = LiqGuard::new(&p, &long, 100.0);
    assert!(near(guard.stop(95.0), 96.9));
    assert!(near(guard.stop(98.0), 98.0));
    // A short's mirror multiplies: liquidation near 105, 2 % under it is 102.9.
    let (short, p) = guarded(true, 20.0, 2.0);
    let guard = LiqGuard::new(&p, &short, 100.0);
    assert!(near(guard.stop(105.0), 102.9));
    assert!(near(guard.stop(101.0), 101.0));
}

/// At a low leverage the liquidation is too far to reach the strategy's stop: nothing moves.
#[test]
fn a_far_liquidation_leaves_the_stop_alone() {
    let (long, p) = guarded(false, 5.0, 2.0);
    assert!(near(LiqGuard::new(&p, &long, 100.0).stop(97.0), 97.0));
}

/// The rule needs its switch, a stop to move, and a leverage with a liquidation price.
#[test]
fn the_guard_is_off_without_its_inputs() {
    let (long, p) = guarded(false, 5.0, 0.0);
    assert_eq!(LiqGuard::new(&p, &long, 100.0).stop_bound(), None);

    let (mut unlevered, p) = guarded(false, 5.0, 50.0);
    unlevered.leverage = None;
    assert_eq!(LiqGuard::new(&p, &unlevered, 100.0).stop_bound(), None);

    // A long at a leverage of 1 has no liquidation price; its short has one at twice the buy.
    let (spot_long, p) = guarded(false, 1.0, 50.0);
    assert_eq!(LiqGuard::new(&p, &spot_long, 100.0).stop_bound(), None);
    let (spot_short, p) = guarded(true, 1.0, 50.0);
    assert!(near(
        LiqGuard::new(&p, &spot_short, 100.0).stop(110.0),
        100.2
    ));

    let (long, mut p) = guarded(false, 5.0, 50.0);
    p.use_stop_loss = false;
    assert_eq!(LiqGuard::new(&p, &long, 100.0).stop_bound(), None);
}

/// `DontSellBelowLiq` holds a sale at the core's liquidation estimate: 100 / 1.19 for a long at
/// a leverage of 5, 119 for a short; off at a leverage of 1 and without the switch.
#[test]
fn the_sale_floor_is_the_liquidation_estimate() {
    let (long, mut p) = guarded(false, 5.0, 0.0);
    assert_eq!(LiqGuard::new(&p, &long, 100.0).sale_floor(), None);
    p.dont_sell_below_liq = true;
    let floor = LiqGuard::new(&p, &long, 100.0).sale_floor().expect("floor");
    assert!(near(floor, 100.0 / 1.19));

    let (short, mut p) = guarded(true, 5.0, 0.0);
    p.dont_sell_below_liq = true;
    let floor = LiqGuard::new(&p, &short, 100.0)
        .sale_floor()
        .expect("floor");
    assert!(near(floor, 119.0));

    let (unlevered, mut p) = guarded(false, 1.0, 0.0);
    p.dont_sell_below_liq = true;
    assert_eq!(LiqGuard::new(&p, &unlevered, 100.0).sale_floor(), None);
}

/// The walk stops at the guarded level, not at `StopLoss`: a dip to 99.7 is far above a stop of
/// −6.5 % and past the guard's 99.8. Without the guard the trade would run to the tape's end.
#[test]
fn the_walk_stops_at_the_guarded_level() {
    let ticks = tape(&[(10, 100.0), (300, 99.7), (900, 99.7)]);
    let (d, mut p) = guarded(false, 5.0, 50.0);
    p.stop_loss_pct = -6.5;
    let guarded_walk = walk(&d, &ticks, fill(), 110.0, &p);
    assert_eq!(guarded_walk.exit.kind, ExitKind::Stop);
    assert!(near(guarded_walk.stop_level.expect("stop"), 99.8));

    p.stop_above_liq_pct = 0.0;
    let plain = walk(&d, &ticks, fill(), 110.0, &p);
    assert_eq!(plain.exit.kind, ExitKind::OpenAtWindowEnd);
}
