//! Market-stop fills retain fire-time verdict routing and price the walk at arrival.

use super::*;
use crate::db::tuner::ticks::{Deltas, ModelSettings};
use crate::feed::types::{Side, Tick};

/// Routing on arrival instead of fire would replace this on-time stop with the take line.
#[test]
fn default_market_stop_routes_on_fire_and_prices_the_market_print() {
    let mut d = Deal {
        report_uid: 1,
        core_uid: 7,
        core_name: String::new(),
        strategy_id: 42,
        kind: "MoonShot".into(),
        coin: "TEST".into(),
        buy_ms: 0,
        close_ms: 250,
        buy_price: 100.0,
        sell_price: 97.0,
        spent: 1000.0,
        is_short: false,
        sell_reason: "StopLoss".into(),
        fact_pnl: -30.0,
        sizing: None,
        pnl_pct: false,
        profit: None,
        deltas: Deltas::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: None,
        fact_modifier: None,
        hook_depth_pct: None,
        drops_detect: None,
        hook_stated_take_pct: None,
        strike_detect: None,
        step_lag_ms: 0.0,
        round_trip_ms: None,
        stop_anchor: None,
        delta_track: None,
        bars: None,
        own_entry: None,
        buy_set_ms: None,
        corridor: None,
        entry_placed: None,
        gap: None,
    };
    let p = ExitParams {
        fast_stop_loss: true,
        stop_loss_pct: -1.0,
        model: ModelSettings {
            latency_ms: 300.0,
            point_time_ms: 1,
            ..ModelSettings::default()
        },
        ..ExitParams::default()
    };
    let ticks = [
        Tick {
            time_ms: 10.0,
            price: 98.0,
            qty: 1.0,
            side: Side::Buy,
        },
        Tick {
            time_ms: 555.0,
            price: 97.0,
            qty: 1.0,
            side: Side::Sell,
        },
    ];
    let walked = ExitModel::new(&p).walk(
        &d,
        &ticks,
        Fill {
            t_ms: 0,
            price: 100.0,
        },
    );
    assert_eq!((walked.exit.t_ms, walked.exit.price), (555, 97.0));
    let verdict = verify(&d, &ticks, &EntryParams::Fact, &p, None, None);
    assert_eq!(verdict.exit_kind, Some(ExitKind::Stop));
    assert_eq!(verdict.exit, Some(true), "{verdict:?}");
    // A non-stop fact removes the fact_stopped routing override.
    d.sell_reason = "Auto Price Down".into();
    let verdict = verify(&d, &ticks, &EntryParams::Fact, &p, None, None);
    assert_eq!(verdict.exit_kind, Some(ExitKind::Stop));
}

/// A long bought at 100 at 0 whose Exit line holds the take placement at 101 first.
fn time_stop_deal(is_short: bool) -> Deal {
    Deal {
        core_name: "CORE_A".into(),
        coin: "XALPHA".into(),
        buy_ms: 0,
        close_ms: 61_000,
        buy_price: 100.0,
        sell_price: 100.0,
        is_short,
        sell_reason: "StopLoss".into(),
        archived_take: Some(if is_short { 99.0 } else { 101.0 }),
        ..crate::db::tuner::ticks::tests::deal()
    }
}

/// `StopLoss` +50 after `StopLossDelay` 60: a book-watching time stop.
fn time_stop(stop_loss_pct: f64, delay_s: f64) -> ExitParams {
    ExitParams {
        stop_loss_pct,
        stop_loss_delay_s: delay_s,
        fast_stop_loss: false,
        ..ExitParams::default()
    }
}

/// The take placement at +1 % stands under a +50 % stop level and so reaches it already; the
/// panic replacement at the buy-side price after the delay is the activation.
#[test]
fn time_stop_activation_skips_the_initial_take() {
    let d = time_stop_deal(false);
    let line = [(50, 101.0), (60_100, 99.9)];
    assert_eq!(
        archived_stop_jump(&d, &time_stop(50.0, 60.0), 150.0, Some(&line)),
        Some(60_100)
    );
    // No delay: the take-placement skip alone holds the first move back.
    let line = [(50, 101.0), (2_000, 99.9)];
    assert_eq!(
        archived_stop_jump(&d, &time_stop(50.0, 0.0), 150.0, Some(&line)),
        Some(2_000)
    );
    // The immediate fast stop's first placement is its own order.
    let immediate = ExitParams {
        fast_stop_loss: true,
        ..time_stop(50.0, 0.0)
    };
    assert_eq!(
        archived_stop_jump(&d, &immediate, 150.0, Some(&line)),
        Some(50)
    );
}

/// Every re-placement of the take at its archived price is skipped, not only the first: the
/// panic replacement after them is the activation.
#[test]
fn time_stop_activation_skips_a_later_take_replacement() {
    let d = time_stop_deal(false);
    // A move off the take and back onto it after the delay: that re-placement is no stop.
    let line = [(0, 101.0), (30_000, 100.5), (61_000, 101.0), (62_000, 99.9)];
    assert_eq!(
        archived_stop_jump(&d, &time_stop(50.0, 60.0), 150.0, Some(&line)),
        Some(62_000)
    );
}

/// A positive stop with no archived take has nothing to skip: the first move past the level
/// once the delay ran out is the activation.
#[test]
fn positive_stop_without_an_archived_take_uses_the_delay_gate_only() {
    let mut d = time_stop_deal(false);
    d.archived_take = None;
    let line = [(50, 101.0), (61_000, 100.8), (62_000, 99.9)];
    assert_eq!(
        archived_stop_jump(&d, &time_stop(50.0, 60.0), 150.0, Some(&line)),
        Some(61_000)
    );
}

/// A move reaching the level before `StopLossDelay` ran out is not the activation.
#[test]
fn time_stop_activation_respects_stop_loss_delay() {
    let d = time_stop_deal(false);
    let p = time_stop(-1.0, 60.0);
    let line = [(50, 101.0), (30_000, 98.5), (61_000, 98.0)];
    assert_eq!(archived_stop_jump(&d, &p, 99.0, Some(&line)), Some(61_000));
}

/// A plain negative stop without a delay fires at the first move past it, as before.
#[test]
fn negative_stop_activation_unchanged() {
    let mut d = time_stop_deal(false);
    d.archived_take = Some(101.0);
    let line = [(0, 101.0), (1_000, 100.8), (3_600, 94.9), (4_040, 94.0)];
    assert_eq!(
        archived_stop_jump(&d, &time_stop(-5.0, 0.0), 95.0, Some(&line)),
        Some(3_600)
    );
}

/// A short's time stop: the take sits below the buy, the level too.
#[test]
fn short_time_stop_activation_mirrored() {
    let d = time_stop_deal(true);
    let line = [(50, 99.0), (60_100, 100.1)];
    assert_eq!(
        archived_stop_jump(&d, &time_stop(50.0, 60.0), 50.0, Some(&line)),
        Some(60_100)
    );
    let line = [(50, 99.0), (2_000, 100.1)];
    assert_eq!(
        archived_stop_jump(&d, &time_stop(50.0, 0.0), 50.0, Some(&line)),
        Some(2_000)
    );
}

/// A long fast stop -1 % fired at 250 whose market sale lands on a 90 bid, under the panic
/// sell's `AllowedDrop` -2 floor (98): the fact's stop, and the tape.
fn past_floor_case() -> (Deal, ExitParams, Vec<Tick>) {
    let d = Deal {
        close_ms: 400,
        sell_price: 90.0,
        ..time_stop_deal(false)
    };
    let p = ExitParams {
        fast_stop_loss: true,
        stop_loss_pct: -1.0,
        allowed_drop_pct: -2.0,
        use_market_order: false,
        model: ModelSettings {
            latency_ms: 0.0,
            point_time_ms: 1,
            ..ModelSettings::default()
        },
        ..ExitParams::default()
    };
    let ticks = vec![
        Tick {
            time_ms: 10.0,
            price: 98.0,
            qty: 1.0,
            side: Side::Buy,
        },
        Tick {
            time_ms: 300.0,
            price: 90.0,
            qty: 1.0,
            side: Side::Sell,
        },
    ];
    (d, p, ticks)
}

/// A stop sale past the floor is no sale the core could make: neither a hit nor a miss.
#[test]
fn past_floor_stop_is_unjudged_sale_unknown() {
    let (d, p, ticks) = past_floor_case();
    let verdict = verify(&d, &ticks, &EntryParams::Fact, &p, None, None);
    assert_eq!(
        verdict.exit_finding,
        ExitFinding::Unjudged(Unjudged::SaleUnknown),
        "{verdict:?}"
    );
    assert_eq!(verdict.exit, None);
}
