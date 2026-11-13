//! Offline recovery scheduling, replica paging and catalog-readiness regressions.

use super::*;
use crate::db::{ReportAxis, ReportStamp};
use crate::feed::{ConnStatus, FeedMsg, MarketDirty, MarketDirtyFlags};

/// Feed three inert sessions through the real drain, preserving their configured election order.
fn currency_sessions(bases: [Option<&str>; 3]) -> (SessionManager, Vec<crate::feed::FeedTx>) {
    let (mut session, first_tx) = super::trade_sound_tests::fixture();
    session.sessions[0].name = "CORE_A".into();
    let mut senders = vec![first_tx];
    for (id, name) in [(2, "CORE_B"), (3, "CORE_C")] {
        let (tx, handle) = crate::feed::trade_sound_test_feed();
        session.store.ensure(id);
        session.sessions.push(CoreSession {
            id,
            name: name.into(),
            group: String::new(),
            conn_sig: 0,
            handle,
        });
        senders.push(tx);
    }
    for (tx, base) in senders.iter().zip(bases) {
        tx.send(FeedMsg::Identity {
            id: crate::feed::ExchangeId::new(1),
            dex: String::new(),
            reported: String::new(),
        })
        .unwrap();
        if let Some(base) = base {
            tx.send(FeedMsg::CoreBase { base: base.into() }).unwrap();
        }
        tx.send(FeedMsg::Status(ConnStatus::Ready)).unwrap();
    }
    session.drain();
    (session, senders)
}

/// Collapsing account currencies would leave the USDC core reading the USDT tape.
#[test]
fn dedup_elects_one_provider_per_account_currency() {
    let (mut session, _tx) = currency_sessions([Some("USDT"), Some("USDT"), Some("USDC")]);
    session.set_open(&[], &[]);
    assert_eq!(session.last_cmd.values().filter(|c| c.0).count(), 2);
    let source = session.market_source();
    assert_eq!(source.provider_of(3), Some(3));
    assert_eq!(source.provider_of(1), source.provider_of(2));
    assert_eq!(source.provider_of(1), Some(1));
}

/// Electing per core instead of per currency would duplicate full-tape subscriptions.
#[test]
fn dedup_single_currency_keeps_one_full_tape() {
    let (mut session, _tx) = currency_sessions([Some("USDT"); 3]);
    session.set_open(&[], &[]);
    assert_eq!(session.last_cmd.values().filter(|c| c.0).count(), 1);
    assert_eq!(session.market_source().provider_of(3), Some(1));
}

/// A lone unavailable USDC member must not fail over to another currency's incomplete tape.
#[test]
fn dedup_usdc_provider_failover_stays_in_its_currency() {
    let (mut session, tx) = currency_sessions([Some("USDT"), Some("USDT"), Some("USDC")]);
    session.set_open(&[], &[]);
    tx[2]
        .send(FeedMsg::Status(ConnStatus::Disconnected))
        .unwrap();
    session.drain();
    session.set_open(&[], &[]);
    assert_eq!(session.market_source().provider_of(3), Some(3));
    assert_eq!(session.last_cmd.values().filter(|c| c.0).count(), 2);
}

/// Currency grouping must not deduplicate tapes when Separate mode is selected.
#[test]
fn per_core_mode_ignores_currency() {
    let (mut session, _tx) = currency_sessions([Some("USDT"), Some("USDT"), Some("USDC")]);
    session.set_market_mode(crate::market::MarketDataMode::PerCore);
    session.set_open(&[], &[]);
    for id in 1..=3 {
        assert_eq!(session.market_source().provider_of(id), Some(id));
    }
    assert_eq!(session.last_cmd.values().filter(|c| c.0).count(), 3);
}

/// Switching modes must keep the open USDC chart subscribed on its own currency's tape.
#[test]
fn separate_to_shared_keeps_an_open_usdc_chart_on_the_usdc_core() {
    let (mut session, _tx) = currency_sessions([Some("USDT"), Some("USDT"), Some("USDC")]);
    let desired = [(3, "XALPHA-USDC".into())];
    session.set_market_mode(crate::market::MarketDataMode::PerCore);
    session.set_open(&desired, &desired);
    session.set_market_mode(crate::market::MarketDataMode::Dedup);
    session.set_open(&desired, &desired);
    assert_eq!(session.market_source().provider_of(3), Some(3));
    assert!(session.wanted[&3].contains("XALPHA-USDC"));
    assert!(
        !session
            .wanted
            .get(&1)
            .is_some_and(|markets| markets.contains("XALPHA-USDC"))
    );
    assert_eq!(
        session.last_cmd[&3],
        (true, vec!["XALPHA-USDC".into()], vec!["XALPHA-USDC".into()])
    );
}

/// A late CoreBase must leave the heuristic group and subscribe its newly known currency tape.
#[test]
fn late_core_base_moves_a_core_into_its_currency_group() {
    let (mut session, tx) = currency_sessions([Some("USDT"), Some("USDT"), None]);
    session.set_open(&[], &[]);
    assert_eq!(session.market_source().provider_of(3), Some(1));
    assert_eq!(session.last_cmd.values().filter(|c| c.0).count(), 1);
    tx[2]
        .send(FeedMsg::CoreBase {
            base: "USDC".into(),
        })
        .unwrap();
    session.drain();
    session.set_open(&[], &[]);
    assert_eq!(session.market_source().provider_of(3), Some(3));
    assert_eq!(session.last_cmd.values().filter(|c| c.0).count(), 2);
}

/// A later resolved or empty page must not erase the protecting identity skipped earlier.
#[test]
fn skipped_rows_keep_the_listing_incomplete() {
    let mut listing = ListingProgress::default();
    listing.page(1000, 1000);
    assert!(listing.complete());
    listing.page(1000, 999);
    listing.page(1000, 1000);
    listing.page(0, 0);
    assert!(!listing.complete());
    let mut complete = ListingProgress::default();
    complete.page(1000, 1000);
    complete.page(40, 40);
    assert!(complete.complete());
}

/// Older replicas without the emulator column must remain readable, while null means plain.
#[test]
fn catchup_sql_reads_emulator_flag() {
    for with_column in [false, true] {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE orders_rep(core_uid INTEGER, newrecid INTEGER, coin TEXT, buydate INTEGER, closedate INTEGER); INSERT INTO orders_rep VALUES(1,1,'XALPHA',10,20),(1,2,'XALPHA',10,20),(1,3,'XALPHA',10,20);").unwrap();
        if with_column {
            conn.execute_batch("ALTER TABLE orders_rep ADD COLUMN emulator INTEGER; UPDATE orders_rep SET emulator=1 WHERE newrecid=1; UPDATE orders_rep SET emulator=0 WHERE newrecid=2;").unwrap();
        }
        let rows = catchup_page(&conn, &catchup_sql(&conn).unwrap(), 1, 0, None).unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| (row.rec_id, row.emulator))
                .collect::<Vec<_>>(),
            vec![(3, false), (2, false), (1, with_column)]
        );
    }
}

/// Losing the sync trigger or hourly boundary would leave closed trades without recovery.
#[test]
fn sync_edge_and_hourly_boundary_are_due() {
    let now = Instant::now();
    assert!(catchup_due(None, now, false));
    assert!(catchup_due(Some(now), now, true));
    assert!(!catchup_due(
        Some(now),
        now + Duration::from_secs(3599),
        false
    ));
    assert!(catchup_due(
        Some(now),
        now + Duration::from_secs(3600),
        false
    ));
}

/// Listing must admit 400-day closes, while open rows and expired closes remain excluded.
#[test]
fn mapping_excludes_open_and_old_rows() {
    let _guard = crate::market::tape_recorder::catchup::tests::SWITCH_LOCK
        .lock()
        .unwrap();
    let _restore = crate::market::tape_recorder::catchup::tests::RestoreKeep::new();
    crate::market::tape_recorder::catchup::set_keep(1825, true);
    let now = 4000 * 86_400_000;
    let axis = ReportAxis::identity_core_local();
    let mut row = CatchupRow {
        rec_id: 4,
        coin: "SYN".into(),
        buy: ReportStamp::Seconds(1),
        close: ReportStamp::Millis(now - 400 * 86_400_000),
        closedate: 1,
        emulator: false,
    };
    assert!(catchup_stamps(&row, &axis, 1, now).is_some());
    row.close = ReportStamp::Millis(now - 1826 * 86_400_000);
    assert!(catchup_stamps(&row, &axis, 1, now).is_none());
    row.closedate = 0;
    row.close = ReportStamp::Millis(now);
    assert!(catchup_stamps(&row, &axis, 1, now).is_none());
}

/// A report sync before the catalog arrives must not permanently reject its coins.
#[test]
fn catalog_pending_does_not_latch_unknown() {
    let mut latched = HashSet::new();
    assert!(!latch_unknown(&mut latched, "COIN_A", false, true));
    assert!(!latch_unknown(&mut latched, "COIN_A", true, false));
    assert!(latched.is_empty());
}

/// An actually unknown coin must count once across all later hourly passes.
#[test]
fn ready_unknown_coin_latches_once() {
    let mut latched = HashSet::new();
    assert!(latch_unknown(&mut latched, "COIN_A", true, true));
    assert!(!latch_unknown(&mut latched, "COIN_A", true, true));
    assert_eq!(latched.len(), 1);
}

/// Repeated unresolved trades of one coin must retain one diagnostic key, not every trade ID.
#[test]
fn station_unknown_coin_retention_is_independent_of_deal_count() {
    let mut latched = HashSet::new();
    let started = Instant::now();
    for _ in 0..100_000 {
        latch_unknown(&mut latched, "COIN_A", true, true);
    }
    println!(
        "unknown 100000: {:.3} ms, keys={}",
        started.elapsed().as_secs_f64() * 1_000.0,
        latched.len()
    );
    assert_eq!(latched.len(), 1);
}

/// An old worker completing after reconnect must not retain a removed core's diagnostic keys.
#[test]
fn retired_unknown_coin_latches_cannot_rejoin_the_session() {
    let (mut session, _tx) = super::trade_sound_tests::fixture();
    let old_listing = session.catchup_unknown.entry(1).or_default().clone();
    session.clear_core_coordination(1);
    latch_unknown(&mut old_listing.lock().unwrap(), "COIN_A", true, true);
    assert!(!session.catchup_unknown.contains_key(&1));
    let new_listing = session.catchup_unknown.entry(1).or_default().clone();
    assert!(!Arc::ptr_eq(&old_listing, &new_listing));
    assert!(new_listing.lock().unwrap().is_empty());
}

/// Equal close times must not starve the rows after the first thousand replica IDs.
#[test]
fn keyset_pages_cover_equal_close_times_and_skip_deleted() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER, newrecid INTEGER, coin TEXT, buydate INTEGER, closedate INTEGER, deleted INTEGER);
        WITH RECURSIVE ids(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM ids WHERE n < 1005)
        INSERT INTO orders_rep SELECT 1, n, 'SYN', 10, 20, 0 FROM ids;
        INSERT INTO orders_rep VALUES (1,1006,'SYN',10,20,1),(1,1007,'SYN',10,0,0),(1,1008,'SYN',10,19,0);").unwrap();
    let sql = catchup_sql(&conn).unwrap();
    let first = catchup_page(&conn, &sql, 1, 20, None).unwrap();
    assert_eq!(first.len(), 1000);
    assert_eq!(first[0].rec_id, 1005);
    let last = first.last().unwrap();
    let second = catchup_page(&conn, &sql, 1, 20, Some((last.closedate, last.rec_id))).unwrap();
    assert_eq!(
        second.iter().map(|row| row.rec_id).collect::<Vec<_>>(),
        vec![5, 4, 3, 2, 1]
    );
}

/// A partially replicated row must not abort the whole page and starve valid neighboring trades.
#[test]
fn nullable_partial_rows_do_not_abort_replica_pages() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER, newrecid INTEGER, coin TEXT, buydate INTEGER, closedate INTEGER);
        INSERT INTO orders_rep VALUES (1,2,NULL,NULL,20),(1,1,'SYN',10,20);").unwrap();
    let rows = catchup_page(&conn, &catchup_sql(&conn).unwrap(), 1, 20, None).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].coin, "");
    assert!(catchup_stamps(&rows[0], &ReportAxis::identity_core_local(), 1, 20_000).is_none());
    assert_eq!(
        catchup_stamps(&rows[1], &ReportAxis::identity_core_local(), 1, 20_000),
        Some((10_000, 20_000))
    );
}

/// A core-local close near the depth boundary must be judged on UTC rather than raw seconds.
#[test]
fn depth_bound_and_millisecond_stamps_follow_measured_axis() {
    let _guard = crate::market::tape_recorder::catchup::tests::SWITCH_LOCK
        .lock()
        .unwrap();
    let _restore = crate::market::tape_recorder::catchup::tests::RestoreKeep::new();
    crate::market::tape_recorder::catchup::set_keep(1825, true);
    let now = 4000 * 86_400_000;
    let cutoff = crate::market::tape_recorder::catchup::edge_ms(now);
    let axis = ReportAxis::from_measured(
        HashMap::from([(
            1,
            vec![crate::db::OffsetSegment {
                from_utc: 0,
                offset_secs: 7200,
            }],
        )]),
        chrono_tz::UTC,
    );
    assert_eq!(axis.from_utc(cutoff / 1000, 1), cutoff / 1000 + 7200);
    let row = CatchupRow {
        rec_id: 4,
        coin: "SYN".into(),
        buy: ReportStamp::Seconds((cutoff - 60_000) / 1000 + 7200),
        close: ReportStamp::Millis(cutoff + 7_200_001),
        closedate: cutoff / 1000 + 7200,
        emulator: false,
    };
    assert_eq!(
        catchup_stamps(&row, &axis, 1, now),
        Some((cutoff - 60_000, cutoff + 1))
    );
}

/// A boundary crossing close dates must retain the exact cutoff row without repeating IDs.
#[test]
fn keyset_crosses_close_dates_and_includes_cutoff() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER, newrecid INTEGER, coin TEXT, buydate INTEGER, closedate INTEGER);
        WITH RECURSIVE ids(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM ids WHERE n < 1005)
        INSERT INTO orders_rep SELECT 1, n, 'SYN', 10, CASE WHEN n > 7 THEN 21 ELSE 20 END FROM ids;
        INSERT INTO orders_rep VALUES (1,2000,'SYN',10,19);").unwrap();
    let sql = catchup_sql(&conn).unwrap();
    let first = catchup_page(&conn, &sql, 1, 20, None).unwrap();
    let last = first.last().unwrap();
    assert_eq!((last.closedate, last.rec_id), (20, 6));
    let second = catchup_page(&conn, &sql, 1, 20, Some((last.closedate, last.rec_id))).unwrap();
    assert_eq!(
        second
            .iter()
            .map(|row| (row.closedate, row.rec_id))
            .collect::<Vec<_>>(),
        vec![(20, 5), (20, 4), (20, 3), (20, 2), (20, 1)]
    );
}

/// Running the listing inline would block session drain; concurrent triggers must share one guard.
#[test]
fn listing_is_background_and_single_per_core() {
    let caller = std::thread::current().id();
    let running = Arc::new(AtomicBool::new(false));
    let (started, rx) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    assert!(spawn_catchup(running.clone(), move || {
        started.send(std::thread::current().id()).unwrap();
        wait.recv().unwrap();
    }));
    assert_ne!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), caller);
    assert!(!spawn_catchup(running.clone(), || panic!(
        "duplicate listing"
    )));
    release.send(()).unwrap();
}
/// Unattributable rows do not own a market window and cannot indefinitely veto maintenance.
#[test]
fn permanently_unattributable_rows_do_not_block_the_listing() {
    let mut listing = ListingProgress::default();
    // One unknown coin and one invalid stamp among otherwise resolved rows.
    listing.resolved_page(1000, 998, 2);
    listing.resolved_page(40, 40, 0);
    assert!(listing.complete());
    listing.resolved_page(1, 0, 0);
    assert!(
        !listing.complete(),
        "a transient catalog gap still protects tape"
    );
}

/// A listed row is tuner-usable only when BOTH stamps came from the millisecond columns, the
/// same gate the tuner's deal read applies. Treating a seconds fallback as usable would keep
/// fetching tape for the pre-millisecond history the switch exists to skip.
#[test]
fn tuner_can_replay_needs_both_millisecond_stamps() {
    let row = |buy, close| CatchupRow {
        rec_id: 4,
        coin: "SYN".into(),
        buy,
        close,
        closedate: 2,
        emulator: false,
    };
    assert!(tuner_can_replay(&row(
        ReportStamp::Millis(1_000),
        ReportStamp::Millis(2_000)
    )));
    assert!(!tuner_can_replay(&row(
        ReportStamp::Seconds(1),
        ReportStamp::Millis(2_000)
    )));
    assert!(!tuner_can_replay(&row(
        ReportStamp::Millis(1_000),
        ReportStamp::Seconds(2)
    )));
}

/// A status-only drain must request a sound scan.
///
/// The detect scanner seeds its cursor on the first visit and stays silent. Leaving
/// `Status` out of the gate would make the first live detect that seed, so a connect
/// followed by a detect would play nothing.
#[test]
fn status_drain_marks_sound_inputs() {
    let (mut session, tx) = super::trade_sound_tests::fixture();
    tx.send(FeedMsg::Status(ConnStatus::Ready)).unwrap();
    assert!(session.drain().sound_inputs);
}

/// Catalog delivery cannot depend on an open market, and price bursts must not wake catalog walks.
#[test]
fn catalog_wakes_are_independent_of_prices_and_open_markets() {
    let (mut session, tx) = super::trade_sound_tests::fixture();
    assert!(session.wanted.is_empty());
    tx.send(FeedMsg::MarketCatalogChanged).unwrap();
    let stats = session.drain();
    assert!(stats.any && stats.market_catalog);
    assert!(!stats.market_data && !stats.ui_state && !stats.sound_inputs);
    for _ in 0..100 {
        tx.send(FeedMsg::MarketDataChanged(vec![MarketDirty::new(
            "BTCUSDT",
            MarketDirtyFlags::HISTORY,
        )]))
        .unwrap();
    }
    let stats = session.drain();
    assert!(stats.market_data);
    assert!(!stats.market_catalog);
    assert!(
        !session.drain().market_catalog,
        "catalog wakes are drained, not sticky"
    );
}

/// The real SDK classifier and session drain must publish read-model changes with no chart names.
#[test]
fn retained_market_updates_wake_without_charts_or_backend_heartbeat() {
    use moonproto::Event;
    use moonproto::state::{CandlesSnapshotEvent, MarketsEvent};

    let (mut session, tx) = super::trade_sound_tests::fixture();
    assert!(session.wanted.is_empty());
    let events = [
        Event::Markets(MarketsEvent::PricesUpdated {
            count: 1,
            included_funding: true,
            included_corr: true,
        }),
        Event::Markets(MarketsEvent::StatsUpdated { count: 1 }),
        Event::CandlesSnapshot(CandlesSnapshotEvent::Ready {
            #[cfg(feature = "moonproto-diagnostics")]
            request_uid: 1,
            summary: Default::default(),
        }),
    ];
    for event in events {
        let dirty = crate::feed::live::market_dirty_from_events(&[event], &[], false)
            .expect("retained snapshots change even when no chart is subscribed");
        assert!(dirty.is_empty());
        tx.send(FeedMsg::MarketDataChanged(dirty)).unwrap();
        let stats = session.drain();
        assert!(stats.any && stats.market_data);
        assert!(!stats.market_catalog && !stats.ui_state && !stats.sound_inputs);
        assert!(
            !session.drain().market_data,
            "publication must not become a heartbeat"
        );
    }
    let unchanged = [
        Event::Markets(MarketsEvent::StatsRefreshFailed {
            message: "offline".into(),
        }),
        Event::CandlesSnapshot(CandlesSnapshotEvent::Failed {
            #[cfg(feature = "moonproto-diagnostics")]
            request_uid: None,
            error: "offline".into(),
        }),
    ];
    assert!(crate::feed::live::market_dirty_from_events(&unchanged, &[], false).is_none());
}

/// Mapping changes invalidate routed catalogs even without any chart subscriptions.
#[test]
fn provider_catalog_wake_covers_initial_mapping_failover_and_removal() {
    let (mut session, tx) = super::trade_sound_tests::fixture();
    session.set_market_mode(crate::market::MarketDataMode::PerCore);
    assert!(session.set_open(&[], &[]));
    assert!(!session.set_open(&[], &[]));
    session.set_market_mode(crate::market::MarketDataMode::Dedup);
    assert!(
        session.core_provider.is_empty(),
        "mode changes clear routing before the next election"
    );
    assert!(!session.set_open(&[], &[]));
    tx.send(FeedMsg::Identity {
        id: crate::feed::ExchangeId { code: 1, dex: 0 },
        dex: String::new(),
        reported: "Binance".into(),
    })
    .unwrap();
    session.drain();
    assert!(
        session.set_open(&[], &[]),
        "late identity establishes the provider"
    );
    assert!(!session.set_open(&[], &[]));
    tx.send(FeedMsg::Status(ConnStatus::Ready)).unwrap();
    session.drain();
    let (second_tx, handle) = crate::feed::trade_sound_test_feed();
    session.store.ensure(2);
    session.sessions.push(CoreSession {
        id: 2,
        name: "second".into(),
        group: String::new(),
        conn_sig: 0,
        handle,
    });
    second_tx
        .send(FeedMsg::Identity {
            id: crate::feed::ExchangeId::new(1),
            dex: String::new(),
            reported: "Binance".into(),
        })
        .unwrap();
    second_tx.send(FeedMsg::Status(ConnStatus::Ready)).unwrap();
    session.drain();
    assert!(
        session.set_open(&[], &[]),
        "a second core joins the catalog route"
    );
    assert_eq!(session.core_provider.get(&2), Some(&1));
    tx.send(FeedMsg::Status(ConnStatus::Disconnected)).unwrap();
    session.drain();
    assert!(
        session.set_open(&[], &[]),
        "failover must invalidate the old provider's catalog"
    );
    assert_eq!(session.core_provider.get(&1), Some(&2));
    assert!(!session.set_open(&[], &[]));
    session.sessions.clear();
    assert!(
        session.set_open(&[], &[]),
        "removing the last core removes the catalog route"
    );
    assert!(!session.set_open(&[], &[]));
}

/// Trades and prices must not request a sound scan.
///
/// Counting `MarketDataChanged` or `TradeOpened` would walk every core on each price
/// wake. Those scans read detect, order, problem, and config revisions, so a price-only
/// drain has nothing new for them.
#[test]
fn trade_and_price_drain_skips_sound_inputs() {
    let (mut session, tx) = super::trade_sound_tests::fixture();
    tx.send(FeedMsg::MarketDataChanged(vec![MarketDirty::new(
        "BTCUSDT",
        MarketDirtyFlags::HISTORY,
    )]))
    .unwrap();
    tx.send(FeedMsg::TradeOpened {
        rec_id: 7,
        coin: "BTC".into(),
        quote: "USDT".into(),
        buy: ReportStamp::Seconds(1),
        emulator: None,
    })
    .unwrap();
    assert!(!session.drain().sound_inputs);
}

/// A detect batch must request a sound scan.
///
/// Dropping `Detects` from the gate would leave a new detect silent until some other
/// sound message happened to arrive.
#[test]
fn detects_drain_marks_sound_inputs() {
    let (mut session, tx) = super::trade_sound_tests::fixture();
    tx.send(FeedMsg::Detects(Vec::new())).unwrap();
    assert!(session.drain().sound_inputs);
}

/// Health samples belong to the telemetry consumer, even when their metrics change.
#[test]
fn telemetry_drain_retains_every_sample_without_waking_unrelated_views() {
    let (mut session, tx) = super::trade_sound_tests::fixture();
    for (stamp, cpu, revision) in [(1000, 10, 1), (2000, 10, 1), (3000, 20, 2)] {
        tx.send(FeedMsg::SysStatus(crate::feed::CoreSysStatus {
            updated_ms: stamp,
            process_cpu_percent: Some(cpu),
            ..Default::default()
        }))
        .unwrap();
        let stats = session.drain();
        assert!(stats.any);
        assert!(!stats.ui_state, "telemetry must use its own publication");
        let core = session.store.core(1).unwrap();
        assert_eq!(core.sys.updated_ms, stamp);
        assert_eq!(core.sys.process_cpu_percent, Some(cpu));
        assert_eq!(core.sys_rev, revision);
    }
    tx.send(FeedMsg::Status(crate::feed::ConnStatus::Ready))
        .unwrap();
    assert!(
        session.drain().ui_state,
        "connection changes still wake their consumers"
    );
}
