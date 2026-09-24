//! Fixtures the sell line's section tests share: a flat deal, a fill at 100, a tape of prints.

use super::ExitParams;
use crate::db::tuner::ticks::{Deal, Deltas, Fill, ModelSettings};
use crate::feed::types::Side as TickSide;
use crate::feed::types::Tick;

pub(super) fn tick(t_ms: i64, price: f64) -> Tick {
    Tick {
        time_ms: t_ms as f64,
        price: price as f32,
        qty: 1.0,
        side: TickSide::Buy,
    }
}

pub(super) fn tape(points: &[(i64, f64)]) -> Vec<Tick> {
    points.iter().map(|&(t, p)| tick(t, p)).collect()
}

pub(super) fn deal(short: bool) -> Deal {
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
        stop_anchor: None,
        delta_track: None,
        own_entry: None,
        buy_set_ms: None,
        corridor: None,
        entry_placed: None,
    }
}

pub(super) fn fill() -> Fill {
    Fill {
        t_ms: 0,
        price: 100.0,
    }
}

/// A 1 % take, no latency, and the rule under test.
pub(super) fn params() -> ExitParams {
    ExitParams {
        model: ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
        ..ExitParams::default()
    }
}
