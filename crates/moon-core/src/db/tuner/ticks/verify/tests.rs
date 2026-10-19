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
