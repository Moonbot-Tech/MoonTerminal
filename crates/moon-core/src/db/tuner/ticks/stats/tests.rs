use super::*;
use crate::db::tuner::ticks::Deltas;

fn deal(pnl: f64, spent: f64) -> Deal {
    Deal {
        report_uid: 1,
        core_uid: 7,
        core_name: String::new(),
        strategy_id: 42,
        kind: "MoonShot".into(),
        coin: "ACE".into(),
        buy_ms: 1,
        close_ms: 2,
        buy_price: 1.0,
        sell_price: 1.0,
        spent,
        is_short: false,
        sell_reason: String::new(),
        fact_pnl: pnl,
        profit: None,
        deltas: Deltas::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: None,
        hook_depth_pct: None,
        hook_stated_take_pct: None,
        step_lag_ms: 0.0,
        stop_anchor: None,
        own_entry: None,
        buy_set_ms: None,
        corridor: None,
        entry_placed: None,
    }
}

#[test]
fn fact_stats_tallies_the_rows_in_order() {
    let deals = [deal(10.0, 100.0), deal(-4.0, 200.0), deal(6.0, 300.0)];
    let stats = fact_stats(&deals);
    assert_eq!((stats.n, stats.wins), (3, 2));
    assert!((stats.profit - 12.0).abs() < 1e-9);
    assert!((stats.avg - 4.0).abs() < 1e-9);
    assert!((stats.avg_spent - 200.0).abs() < 1e-9);
    assert!((stats.max_dd - 4.0).abs() < 1e-9, "{}", stats.max_dd);
    let empty = fact_stats(&[]);
    assert_eq!(empty.n, 0);
    assert_eq!(empty.avg_spent, 0.0);
}
