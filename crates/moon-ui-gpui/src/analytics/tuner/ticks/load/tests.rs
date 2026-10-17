//! Core-only rows use the same retention rule at load time and when offered for fetch.

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
    let ticks = (70..=180)
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
    };
    let rows = super::replay_fetched(
        fetched,
        &Default::default(),
        Default::default(),
        None,
        3_710_000,
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
    let _locale = crate::test_locale::force("en");
    let caption =
        super::super::coverage_caption(data.covered(), data.fit(), data.rows.len(), 0, 0, 0);
    assert_eq!(
        caption,
        rust_i18n::t!(
            "analytics.ticks.coverage",
            covered = 1,
            total = 1,
            fit = data.fit(),
            without = 0
        )
        .to_string()
    );
}
