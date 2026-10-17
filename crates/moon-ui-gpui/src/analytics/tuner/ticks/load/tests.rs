//! Core-only rows use the same retention rule at load time and when offered for fetch; a
//! settled miss is carried only while a replay would say the same; stage C's chunked replay
//! gives the rows an unbounded one gives.

mod carry;

use super::unservable_status;
use crate::analytics::tuner::ticks::state::{RowAddress, TapeStatus};
use moon_core::db::tuner::ticks::Deal;
use moon_core::market::trade_replay::TickStatus;

/// Shared load/fetch reach must keep a recent unlisted row out of the terminal refusal state.
#[test]
fn unlisted_load_reach_is_core_aware() {
    let deal = Deal {
        report_uid: 1,
        core_uid: 1,
        core_name: "CORE_X".into(),
        strategy_id: 1,
        kind: "Spread".into(),
        coin: "XALPHA".into(),
        buy_ms: 100_000,
        close_ms: 110_000,
        buy_price: 1.0,
        sell_price: 1.01,
        spent: 100.0,
        is_short: false,
        sell_reason: String::new(),
        fact_pnl: 0.0,
        sizing: None,
        pnl_pct: false,
        profit: None,
        deltas: Default::default(),
        tick: None,
        pre_spike_ask: None,
        archived_take: None,
        fact_modifier: None,
        hook_depth_pct: None,
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
    let address = RowAddress {
        core_uid: 1,
        venue: None,
        exchange_key: moon_core::fixture::unlisted::EXCHANGE_KEY.into(),
        market: "XALPHA".into(),
        btc_market: None,
    };
    assert_eq!(
        unservable_status(&address, &deal, deal.close_ms + 3_600_000),
        None
    );
    assert_eq!(
        unservable_status(&address, &deal, deal.close_ms + 49 * 3_600_000),
        Some(TapeStatus::Refused(TickStatus::OutOfRetention {
            retention_ms: moon_core::market::CORE_ARCHIVE_RETENTION_MS,
        }))
    );

    let mut known = address.clone();
    known.venue = Some(moon_core::venue::Venue {
        brand: moon_core::venue::Brand::Bybit,
        kind: moon_core::venue::MarketKind::Spot,
    });
    for age in [3_600_000, 49 * 3_600_000] {
        assert_eq!(
            unservable_status(&known, &deal, deal.close_ms + age),
            Some(TapeStatus::Refused(TickStatus::NoRoute))
        );
    }

    let window = moon_core::db::tuner::ticks::model_window(&deal, 30_000, i64::MAX).unwrap();
    let ticks: Vec<_> = (70..=180)
        .map(|second| moon_core::feed::types::Tick {
            time_ms: f64::from(second) * 1_000.0,
            price: if second < 110 { 1.0 } else { 1.01 },
            qty: 1.0,
            side: moon_core::feed::types::Side::Buy,
        })
        .collect();
    let row = crate::analytics::tuner::ticks::state::DealRow {
        deal,
        tape: TapeStatus::Missing,
        verdict: None,
        address: Some(std::sync::Arc::new(address)),
        ticks: None,
        entry_line: None,
        held: None,
    };
    // Even a tail shorter than the model pad must preserve a fully held old trade.
    let short = super::Fetched {
        rows: vec![row.clone()],
        tapes: [(
            1,
            (
                ticks
                    .iter()
                    .copied()
                    .filter(|tick| tick.time_ms <= 110_000.0)
                    .collect(),
                moon_core::market::trade_replay::Coverage::one((70_000, 110_000)),
                window,
            ),
        )]
        .into(),
        traces: Default::default(),
        settled: Default::default(),
    };
    let short_rows = super::replay_fetched(
        short,
        &Default::default(),
        Default::default(),
        None,
        500_000_000,
        None,
        None,
    );
    assert_eq!(short_rows[0].tape, TapeStatus::Covered);
    assert_eq!(short_rows[0].held, Some((30_000, 0)));
    assert!(short_rows[0].verdict.is_some());
    let gapped = super::Fetched {
        rows: vec![row.clone()],
        tapes: [(
            1,
            (
                ticks
                    .iter()
                    .copied()
                    .filter(|tick| tick.time_ms <= 110_000.0 || tick.time_ms >= 150_000.0)
                    .collect(),
                moon_core::market::trade_replay::Coverage::from_spans([
                    (70_000, 110_000),
                    (150_000, 180_000),
                ]),
                window,
            ),
        )]
        .into(),
        traces: Default::default(),
        settled: Default::default(),
    };
    let gapped_rows = super::replay_fetched(
        gapped,
        &Default::default(),
        Default::default(),
        None,
        500_000_000,
        None,
        None,
    );
    assert_ne!(
        gapped_rows[0].tape,
        TapeStatus::Covered,
        "a later island cannot waive a hole in the model pad"
    );
    assert!(gapped_rows[0].verdict.is_none());
    let fetched = super::Fetched {
        rows: vec![row],
        tapes: [(
            1,
            (
                ticks,
                moon_core::market::trade_replay::Coverage::one((70_000, 180_000)),
                window,
            ),
        )]
        .into(),
        traces: Default::default(),
        settled: Default::default(),
    };
    let rows = super::replay_fetched(
        fetched,
        &Default::default(),
        Default::default(),
        None,
        3_710_000,
        None,
        None,
    );
    assert_eq!(rows[0].tape, TapeStatus::Covered);
    assert!(rows[0].verdict.is_some());
    let readiness =
        crate::analytics::tuner::ticks::readiness::Readiness::of_rows(&rows, 3_710_000, 60);
    assert_eq!(readiness.covered, 1);
    let data = crate::analytics::tuner::ticks::state::TicksData {
        rows,
        ..Default::default()
    };
    assert_eq!(data.covered(), 1);
}

/// A field-rules edit, a schema default or a selected strategy's value that moved changes the
/// reload's key, so the fast path does not keep the warning, grid and replay inputs of before.
#[test]
fn rules_defaults_and_selection_reach_the_scope_signature() {
    use moon_core::feed::strategy_deps::FieldDeps;
    use std::collections::HashMap;
    use std::sync::Arc;
    let read = moon_core::db::tuner::ticks::DealsRead::default();
    let deps = FieldDeps::parse("\"FieldA\" = \"FieldB=1\"");
    let defaults = HashMap::from([("FieldA".to_string(), 1.0)]);
    let selected = vec![(
        (1, Some(1)),
        Arc::new(HashMap::from([("FieldA".to_string(), "2".to_string())])),
    )];
    let sign = |selected: &[super::SelectedValues], defaults: &HashMap<String, f64>, deps| {
        super::scope_signature(
            &read,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            selected,
            defaults,
            deps,
        )
    };
    let base = sign(&selected, &defaults, &deps);
    let same = deps.clone();
    assert_eq!(base, sign(&selected, &defaults, &same));
    let edited = FieldDeps::parse("\"FieldA\" = \"FieldB=2\"");
    assert_ne!(base, sign(&selected, &defaults, &edited), "rules edited");
    let moved = HashMap::from([("FieldA".to_string(), 3.0)]);
    assert_ne!(base, sign(&selected, &moved, &deps), "default moved");
    let reselected = vec![(
        (1, Some(1)),
        Arc::new(HashMap::from([("FieldA".to_string(), "4".to_string())])),
    )];
    assert_ne!(base, sign(&reselected, &defaults, &deps), "selection moved");
}
