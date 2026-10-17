//! The shortest tail a deal must hold.

use moon_core::db::tuner::ticks::tail::{MAX_MIN_TAIL_S, floor_ms};

use super::{current_s, effective_s, parse_s, reaches};

/// A trail reaches the minimum at it and past it; a shorter one, or no coverage, does not; 0
/// takes every covered row.
#[test]
fn a_trail_reaches_the_minimum_at_and_past_it() {
    assert!(reaches(Some((0, 60_000)), 60));
    assert!(reaches(Some((5_000, 180_000)), 60));
    assert!(!reaches(Some((60_000, 30_000)), 60));
    assert!(!reaches(None, 60));
    assert!(reaches(Some((0, 0)), 0));
}

/// A typed value is whole seconds, clamped to the store's longest margin; anything else is
/// refused.
#[test]
fn a_typed_value_is_whole_seconds_clamped_to_the_store() {
    assert_eq!(parse_s(" 90 "), Some(90));
    assert_eq!(parse_s("0"), Some(0));
    assert_eq!(parse_s("100000"), Some(MAX_MIN_TAIL_S));
    assert_eq!(parse_s("1.5"), None);
    assert_eq!(parse_s("-5"), None);
    assert_eq!(parse_s(""), None);
}

/// Recapping by the terminal margin would mislabel the floor shown in the footer.
#[test]
fn effective_seconds_are_the_core_floor() {
    assert_eq!(i64::from(effective_s()) * 1_000, floor_ms(current_s()));
}

/// Ignoring either source switch would admit a fillable tail or keep unavailable tape waiting.
#[test]
fn short_tail_reach_respects_venue_and_core_switches() {
    use super::super::state::RowAddress;
    use moon_core::venue::{Brand, MarketKind, Venue};
    let address = RowAddress {
        core_uid: 1,
        venue: Some(Venue {
            brand: Brand::Binance,
            kind: MarketKind::Spot,
        }),
        exchange_key: "EX-XX".into(),
        market: "XALPHAUSDT".into(),
        btc_market: None,
    };
    let now = 100 * 86_400_000;
    let old = now - 49 * 3_600_000;
    assert!(super::fillable_at(
        &address,
        1,
        old,
        30_000,
        now,
        (true, false)
    ));
    assert!(!super::fillable_at(
        &address,
        1,
        old,
        30_000,
        now,
        (false, true)
    ));
    assert!(!super::fillable_at(
        &address,
        1,
        now - 3_600_000,
        30_000,
        now,
        (false, false)
    ));
}

/// A station-held minute must fit a thirty-second replay window, clearing the short-tail count.
#[test]
fn station_tail_is_the_rows_exit_horizon_and_clears_the_footer() {
    use super::super::state::{DealRow, RowAddress, TapeStatus};
    use moon_core::db::tuner::ticks::verify::{EntryFinding, ExitFinding, RuleFlags, StopFacts};
    use moon_core::db::tuner::ticks::{Deal, Deltas, Verdict};
    use moon_core::market::trade_replay::{Coverage, TickStatus, replay_window_ms};
    use moon_core::venue::{Brand, MarketKind, Venue};
    use std::sync::Arc;
    let deal = Deal {
        report_uid: 1,
        core_uid: 1,
        core_name: String::new(),
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
        deltas: Deltas::default(),
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
    let mut row = DealRow {
        deal,
        tape: TapeStatus::Covered,
        verdict: Some(Verdict {
            entry: Some(true),
            entry_dev_pct: None,
            exit: Some(true),
            exit_dev_pct: None,
            fill: None,
            exit_kind: None,
            line_points: None,
            entry_finding: EntryFinding::Hit,
            exit_finding: ExitFinding::Hit,
            fill_clock_ms: None,
            rules: RuleFlags::default(),
            stop: StopFacts::default(),
        }),
        address: Some(Arc::new(RowAddress {
            core_uid: 1,
            venue: Some(Venue {
                brand: Brand::Bybit,
                kind: MarketKind::Spot,
            }),
            exchange_key: "synthetic".into(),
            market: "XALPHAUSDT".into(),
            btc_market: None,
        })),
        ticks: None,
        entry_line: None,
        held: Some((30_000, 30_000)),
    };
    assert!(row.fit(), "an old unfillable short tail remains usable");
    assert!(!row.station_candidate());
    assert!(!row.fetchable_at(500_000_000, 60));
    let window = replay_window_ms(100_000, 110_000, 30_000).unwrap();
    let focus = moon_core::db::tuner::ticks::tail::focus_with_tail(&window, 60_000);
    let covered = Coverage::one((70_000, 170_000)).clip(&focus);
    let (from, to) = covered.hull().unwrap();
    row.held = Some((row.deal.buy_ms - from, to - row.deal.close_ms));
    assert_eq!(row.held, Some((30_000, 60_000)));
    assert!(row.fit());
    assert!(!row.station_candidate());
    for status in [
        TickStatus::NoRoute,
        TickStatus::OutOfRetention { retention_ms: 1 },
    ] {
        row.tape = TapeStatus::Refused(status);
        row.held = None;
        assert!(row.station_candidate());
        assert!(!row.fetchable_at(500_000_000, 60));
    }
}
