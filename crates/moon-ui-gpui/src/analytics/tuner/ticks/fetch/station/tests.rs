//! What the autoload asks the station for.

use std::sync::Mutex;

use super::{Filled, ReplayWindow, Unfillable, lacking, note_filed, prefill_rows};

/// Serializes tests that read or write the process-wide unfillable set.
static UNFILLABLE_TEST: Mutex<()> = Mutex::new(());

/// A latched final row stops waiting even if its timestamps still lie inside core retention.
#[test]
fn station_final_tail_no_longer_waits_for_a_source() {
    let _guard = UNFILLABLE_TEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let row = queued_unservable(-91_401);
    let now = row.window.close_ms + 3_600_000;
    super::unfillable().uids.remove(&row.deal.report_uid);
    assert!(super::super::super::tail::fillable_at(
        &row.address,
        row.deal.report_uid,
        row.window.close_ms,
        30_000,
        now,
        (false, true)
    ));
    super::unfillable().uids.insert(row.deal.report_uid);
    let fillable = super::super::super::tail::fillable_at(
        &row.address,
        row.deal.report_uid,
        row.window.close_ms,
        30_000,
        now,
        (false, true),
    );
    super::unfillable().uids.remove(&row.deal.report_uid);
    assert!(!fillable);
}

/// A disk-held row must not invalidate the view when the station filed no new pieces.
#[test]
fn already_held_rows_do_not_bump_the_station_filing_generation() {
    let before = super::filed_generation();
    let (_, held, _, filled) = prefill_rows(
        vec![trade("HELD", 100_000, 110_000)],
        |row| row.clone(),
        |_, _, _, _| Some(vec![(0, 1_000_000)]),
        |_, _| false,
        |_| panic!("held row must not reach the station"),
    );
    assert_eq!(held, 1);
    note_filed(filled);
    assert_eq!(super::filed_generation(), before);
}

/// Remembering an unservable station miss prevents repeated autoload requests and load counts.
#[test]
fn station_misses_are_excluded_until_filing_or_finalisation_changes() {
    let mut refused = Unfillable::default();
    refused.refresh((7, 2));
    refused.uids.insert(91);
    refused.refresh((7, 2));
    assert!(refused.uids.contains(&91));
    refused.refresh((7, 3));
    assert!(refused.uids.is_empty());
    refused.uids.insert(91);
    refused.refresh((8, 3));
    assert!(refused.uids.is_empty());
}

/// A refused station-only row drops out of the footer and the next automatic prefill.
#[test]
fn an_unservable_row_is_not_offered_or_counted_again() {
    let _guard = UNFILLABLE_TEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    use super::super::super::state::{DealRow, RowAddress, TapeStatus};
    use moon_core::db::tuner::ticks::{Deal, Deltas};
    use moon_core::market::trade_replay::TickStatus;
    use moon_core::venue::{Brand, MarketKind, Venue};
    use std::sync::Arc;
    let row = DealRow {
        deal: Deal {
            report_uid: -91_001,
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
        },
        tape: TapeStatus::Refused(TickStatus::NoRoute),
        verdict: None,
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
        held: None,
    };
    let now = 500_000_000;
    assert!(super::super::station_load_candidate(&row, now));
    super::unfillable().uids.insert(row.deal.report_uid);
    assert!(!super::super::station_load_candidate(&row, now));
    let address = row.address.as_ref().unwrap().clone();
    let window = moon_core::db::tuner::ticks::model_window(&row.deal, 30_000, 60_000).unwrap();
    let source =
        moon_core::market::MarketDataSource::new(moon_core::market::MarketStore::shared(0.0));
    source.set_provider_map(&[(1, 1)].into());
    source.set_provider_exchanges(&[(1, moon_core::feed::ExchangeId::new(0))].into());
    let queued = super::super::job::QueuedRow::new(
        row.deal.clone(),
        address,
        source.replay_address(1).unwrap(),
        moon_core::market::trade_replay::venue_caps::TickValue::Base,
        window,
        super::super::job::RowOrigin::Autoload,
    );
    let (remaining, _, _) = super::prefill(vec![queued], None, || false);
    assert!(
        remaining.is_empty(),
        "the next autoload must not offer the row again"
    );
    super::unfillable().uids.remove(&row.deal.report_uid);
}

/// An empty station answer for a row no later source can fetch must be remembered.
///
/// Deleting the refusal extend in `commit_station_answer` leaves the uid out, so the next
/// autoload asks the station for that row again.
#[test]
fn empty_station_answer_is_remembered_when_nothing_else_can_fetch() {
    let _guard = UNFILLABLE_TEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let queued = queued_unservable(-91_302);
    let uid = queued.deal.report_uid;
    super::unfillable().uids.remove(&uid);
    let (rows, _, _, filled) = prefill_rows(
        vec![queued],
        |row| {
            (
                row.address.exchange_key.clone(),
                row.address.market.clone(),
                row.window,
            )
        },
        |_, _, _, _| None,
        |_, _| false,
        |_| Filled {
            markets: 1,
            ..Default::default()
        },
    );
    assert_eq!(
        rows.len(),
        1,
        "an empty answer must leave the row for refusal memory"
    );
    super::commit_station_answer(&rows, filled, false);
    assert!(
        super::unfillable().uids.contains(&uid),
        "an empty station answer must remember a row nothing else can fetch"
    );
    super::unfillable().uids.remove(&uid);
}

/// A Bybit row from 1970: no trade route, and the missing tape is outside the core horizon.
fn queued_unservable(report_uid: i64) -> super::super::job::QueuedRow {
    use super::super::super::state::{DealRow, RowAddress, TapeStatus};
    use moon_core::db::tuner::ticks::{Deal, Deltas};
    use moon_core::market::trade_replay::TickStatus;
    use moon_core::venue::{Brand, MarketKind, Venue};
    use std::sync::Arc;
    let row = DealRow {
        deal: Deal {
            report_uid,
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
        },
        tape: TapeStatus::Refused(TickStatus::NoRoute),
        verdict: None,
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
        held: None,
    };
    let address = row.address.as_ref().unwrap().clone();
    let window = moon_core::db::tuner::ticks::model_window(&row.deal, 30_000, 60_000).unwrap();
    let source =
        moon_core::market::MarketDataSource::new(moon_core::market::MarketStore::shared(0.0));
    source.set_provider_map(&[(1, 1)].into());
    source.set_provider_exchanges(&[(1, moon_core::feed::ExchangeId::new(0))].into());
    super::super::job::QueuedRow::new(
        row.deal,
        address,
        source.replay_address(1).unwrap(),
        moon_core::market::trade_replay::venue_caps::TickValue::Base,
        window,
        super::super::job::RowOrigin::Autoload,
    )
}

/// Dropping by the replay pad would skip the station and leave thirty-second rows short forever.
#[test]
fn station_fills_short_tail_before_the_remaining_rows_are_queued() {
    use std::cell::Cell;
    let end = Cell::new(140_000);
    let offered = Cell::new(0);
    let (rest, held, from_station, _) = prefill_rows(
        vec![
            trade("FILLED", 100_000, 110_000),
            trade("VENUE", 100_000, 110_000),
        ],
        |row| row.clone(),
        |_, market, _, _| {
            Some(vec![(
                70_000,
                if market == "FILLED" {
                    end.get()
                } else {
                    140_000
                },
            )])
        },
        |_, _| false,
        |rows| {
            offered.set(rows.len());
            let wants = lacking(
                rows,
                |row| row.clone(),
                |_, _, _, _| Some(vec![(70_000, 140_000)]),
            );
            assert!(
                wants
                    .iter()
                    .all(|want| want.spans == vec![(140_001, 170_000)])
            );
            end.set(170_000);
            Filled {
                pieces: 1,
                ..Default::default()
            }
        },
    );
    assert_eq!(offered.get(), 2);
    assert_eq!(held, 1);
    assert_eq!(from_station, 1);
    assert_eq!(
        rest.iter().map(|row| row.1.as_str()).collect::<Vec<_>>(),
        vec!["VENUE"]
    );
}

/// A trade `(exchange, market, open_ms, close_ms)` and its window, 30 s either side.
fn trade(market: &str, open_ms: i64, close_ms: i64) -> (String, String, ReplayWindow) {
    let window = moon_core::market::trade_replay::replay_window_ms(open_ms, close_ms, 30_000)
        .expect("a window");
    ("x".to_string(), market.to_string(), window)
}

/// The station is asked for each market once, for the windows of its rows merged, minus what the
/// file already holds; a market the file holds whole is not asked, and a market whose read did not
/// happen is asked whole.
#[test]
fn the_station_is_asked_only_for_what_the_file_lacks() {
    let rows = vec![
        trade("A", 100_000, 110_000),
        trade("A", 105_000, 150_000),
        trade("B", 100_000, 110_000),
        trade("C", 100_000, 110_000),
    ];
    let wants = lacking(
        &rows,
        |row| row.clone(),
        |_, market, from, to| match market {
            // A: the first 20 s held.
            "A" => Some(vec![(from, from + 19_999)]),
            // B: all of it held.
            "B" => Some(vec![(from, to)]),
            // C: the read did not happen.
            _ => None,
        },
    );
    assert_eq!(
        wants
            .iter()
            .map(|w| (w.market.as_str(), w.spans.clone()))
            .collect::<Vec<_>>(),
        vec![
            ("A", vec![(90_000, 210_000)]),
            ("C", vec![(70_000, 170_000)]),
        ]
    );
}
