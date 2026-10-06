// Not `super::*`: the parent's `gpui::*` brings gpui's own `test` attribute, which `#[test]` would
// then name, and it expands into itself.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::super::state::{DealRow, RowAddress, TapeStatus};
use super::drop_refiled;
use moon_core::db::tuner::ticks::{Deal, Deltas};
use moon_core::market::trade_replay::TickStatus;
use moon_core::venue::{Brand, MarketKind, Venue};

fn deal(uid: i64) -> Deal {
    Deal {
        report_uid: uid,
        core_uid: 1,
        core_name: String::new(),
        strategy_id: 1,
        kind: "Spread".into(),
        coin: "PONS".into(),
        buy_ms: 1_000,
        close_ms: 2_000,
        buy_price: 1.0,
        sell_price: 1.01,
        spent: 100.0,
        is_short: false,
        sell_reason: String::new(),
        fact_pnl: 0.0,
        sizing: None,
        pnl_pct: false,
        profit: None,
        deltas: Deltas::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: None,
        fact_modifier: None,
        hook_depth_pct: None,
        hook_stated_take_pct: None,
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
    }
}

fn row(uid: i64, market: &str, tape: TapeStatus) -> DealRow {
    DealRow {
        deal: deal(uid),
        tape,
        verdict: None,
        address: Some(Arc::new(RowAddress {
            core_uid: 1,
            venue: Venue {
                brand: Brand::Bybit,
                kind: MarketKind::Futures,
            },
            exchange_key: "2:00000000".into(),
            market: market.into(),
            btc_market: None,
        })),
        ticks: None,
        entry_line: None,
        held: None,
    }
}

/// The station's prints for PONS landed after its row was refused as unservable: that row is read
/// again; a refused row of another market, a covered row and a walk's refusal of the same market
/// are carried as they were.
#[test]
fn a_refused_row_whose_market_gained_prints_is_not_carried() {
    let judged: HashMap<i64, DealRow> = [
        row(1, "PONSUSDT", TapeStatus::Refused(TickStatus::NoRoute)),
        row(2, "MRVLUSDT", TapeStatus::Refused(TickStatus::NoRoute)),
        row(3, "PONSUSDT", TapeStatus::Covered),
        row(4, "PONSUSDT", TapeStatus::Refused(TickStatus::Failed)),
    ]
    .into_iter()
    .map(|r| (r.deal.report_uid, r))
    .collect();
    let refiled: HashSet<(String, String)> =
        [("2:00000000".to_string(), "PONSUSDT".to_string())].into();
    let kept = drop_refiled(judged, &refiled);
    assert!(!kept.contains_key(&1));
    assert!(kept.contains_key(&2));
    assert!(kept.contains_key(&3));
    // A refusal the venue's walk gave is the fetch job's to retry, not the store's to lift.
    assert!(kept.contains_key(&4));
}
