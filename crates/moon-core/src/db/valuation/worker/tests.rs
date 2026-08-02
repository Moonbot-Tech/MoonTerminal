//! Valuation worker invalidation regression tests.

use super::*;
use crate::db::valuation::{RateOrientation, ResolvedRate};

/// Provider boundary that fails a test if an invalidation-only event touches the network.
struct NoNetwork;

impl SpotRateSource for NoNetwork {
    /// Reject every unexpected provider call.
    ///
    /// Args:
    ///     _provider: Unexpected provider identifier.
    ///     _symbol: Unexpected provider-native symbol.
    ///     _start_minute_utc: Unexpected first UTC minute.
    ///     _end_minute_utc: Unexpected last UTC minute.
    ///
    /// Returns:
    ///     Never returns because invalidation-only work must not fetch market data.
    ///
    /// Panics:
    ///     Always panics to expose an unexpected network boundary crossing.
    fn candles(
        &self,
        _provider: &'static str,
        _symbol: &str,
        _start_minute_utc: i64,
        _end_minute_utc: i64,
    ) -> Result<Vec<super::super::provider::SpotCandle>, FetchFailure> {
        panic!("invalidation events must not request market data")
    }
}

/// Provider boundary proving every canonical route absent.
struct MissingSource;

impl SpotRateSource for MissingSource {
    /// Return a permanent route miss for every requested market.
    ///
    /// Args:
    ///     _provider: Canonical provider identifier.
    ///     _symbol: Provider-native spot symbol.
    ///     _start_minute_utc: First requested UTC minute.
    ///     _end_minute_utc: Last requested UTC minute.
    ///
    /// Returns:
    ///     Permanent missing-route failure.
    fn candles(
        &self,
        _provider: &'static str,
        _symbol: &str,
        _start_minute_utc: i64,
        _end_minute_utc: i64,
    ) -> Result<Vec<super::super::provider::SpotCandle>, FetchFailure> {
        Err(FetchFailure::Missing)
    }
}

/// Store one prepared row for partition and hard-delete assertions.
///
/// Args:
///     store: Open in-memory valuation database.
///     source: Typed or legacy source partition.
///     core_uid: Test core identity.
///     row_id: Test row identity.
fn seed_value(store: &Connection, source: TradeSource, core_uid: i64, row_id: i64) {
    let minute = 1_700_000_040;
    let rate = ResolvedRate {
        quote_ordinal: 8,
        minute_utc: minute,
        rate_usdt: 0.999,
        provider: "fixture".to_string(),
        symbol: "USDCUSDT".to_string(),
        orientation: RateOrientation::Direct,
        candle_open_ms: minute * 1_000,
        candle_close_ms: minute * 1_000 + 59_999,
    };
    let input = TradeInput {
        source,
        core_uid,
        row_id,
        closedate: minute + 5,
        quote_ordinal: 8,
        profit_quote: row_id as f64,
        spent_quote: Some(100.0),
    };
    super::super::store_trade_value(store, &input, &rate, minute * 1_000 + 120_000)
        .expect("seed prepared value");
}

/// `worker.rs:process_event` must invalidate exactly the source/core partition named by reset or
/// legacy-purge events; widening either delete would discard unrelated cached history, while
/// narrowing it would leave stale prepared rows visible after a report database replacement.
#[test]
fn partition_events_delete_only_the_named_source_and_core() {
    let store =
        super::super::open_store(std::path::Path::new(":memory:")).expect("open valuation fixture");
    let reports = Connection::open_in_memory().expect("open report fixture");
    seed_value(&store, TradeSource::Typed, 1, 10);
    seed_value(&store, TradeSource::Typed, 2, 20);
    seed_value(&store, TradeSource::Legacy, 1, 30);
    let mut deferred = BTreeMap::from([
        (
            (TradeSource::Typed.code(), 1, 10),
            TradeInput {
                source: TradeSource::Typed,
                core_uid: 1,
                row_id: 10,
                closedate: current_minute_utc(),
                quote_ordinal: 8,
                profit_quote: 10.0,
                spent_quote: None,
            },
        ),
        (
            (TradeSource::Typed.code(), 2, 20),
            TradeInput {
                source: TradeSource::Typed,
                core_uid: 2,
                row_id: 20,
                closedate: current_minute_utc(),
                quote_ordinal: 8,
                profit_quote: 20.0,
                spent_quote: None,
            },
        ),
    ]);

    let reset = process_event(
        &store,
        &NoNetwork,
        &reports,
        OutboxEvent {
            seq: 1,
            source: TradeSource::Typed,
            core_uid: 1,
            row_id: 0,
            action: OutboxAction::RescanCore,
        },
        &mut deferred,
    );
    assert!(matches!(reset, PrepareResult::Complete { changed: true }));
    assert!(!deferred.contains_key(&(TradeSource::Typed.code(), 1, 10)));
    assert!(deferred.contains_key(&(TradeSource::Typed.code(), 2, 20)));

    let rows = store
        .prepare(
            "SELECT source_kind, core_uid, row_id FROM trade_values ORDER BY source_kind, core_uid",
        )
        .expect("prepare remaining rows")
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .expect("query remaining rows")
        .map(|row| row.expect("decode remaining row"))
        .collect::<Vec<_>>();
    assert_eq!(rows, vec![(0, 2, 20), (1, 1, 30)]);

    let purge = process_event(
        &store,
        &NoNetwork,
        &reports,
        OutboxEvent {
            seq: 2,
            source: TradeSource::Legacy,
            core_uid: 1,
            row_id: 0,
            action: OutboxAction::PurgeLegacy,
        },
        &mut deferred,
    );
    assert!(matches!(purge, PrepareResult::Complete { changed: true }));
    assert_eq!(
        store
            .query_row("SELECT COUNT(*) FROM trade_values", [], |row| row
                .get::<_, i64>(0))
            .expect("count remaining rows"),
        1
    );
}

/// `worker.rs:process_event` must remove one exact prepared identity on a hard delete; retaining it
/// would let a deleted report row reappear in coverage if that identity were later reused.
#[test]
fn hard_delete_removes_one_exact_prepared_identity() {
    let store =
        super::super::open_store(std::path::Path::new(":memory:")).expect("open valuation fixture");
    let reports = Connection::open_in_memory().expect("open report fixture");
    seed_value(&store, TradeSource::Typed, 1, 10);
    seed_value(&store, TradeSource::Typed, 1, 11);
    let mut deferred = BTreeMap::new();

    let deleted = process_event(
        &store,
        &NoNetwork,
        &reports,
        OutboxEvent {
            seq: 1,
            source: TradeSource::Typed,
            core_uid: 1,
            row_id: 10,
            action: OutboxAction::Delete,
        },
        &mut deferred,
    );

    assert!(matches!(deleted, PrepareResult::Complete { changed: true }));
    let remaining = store
        .query_row("SELECT row_id FROM trade_values", [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("read surviving identity");
    assert_eq!(remaining, 11);
}

/// `worker.rs:prefetch_rates` must report a newly cached permanent miss as a visible coverage
/// change; returning false leaves Report and Analytics showing an endless pending count until an
/// unrelated report commit wakes them.
#[test]
fn permanent_missing_rate_is_a_publishable_coverage_change() {
    let store =
        super::super::open_store(std::path::Path::new(":memory:")).expect("open valuation fixture");
    let minute = current_minute_utc() - 120;
    let input = TradeInput {
        source: TradeSource::Typed,
        core_uid: 1,
        row_id: 10,
        closedate: minute + 5,
        quote_ordinal: 8,
        profit_quote: 5.0,
        spent_quote: Some(100.0),
    };

    let changed =
        prefetch_rates(&store, &MissingSource, &[input]).expect("cache canonical permanent miss");

    assert!(changed);
    assert_eq!(
        super::super::cached_rate(&store, 8, minute).expect("read cached miss"),
        Some(CachedRate::PermanentMissing)
    );
}

/// Removing the error-side publication from `worker.rs:settle_prefetch` would leave committed
/// permanent misses or earlier prepared rows invisible while a later provider retry keeps failing.
#[test]
fn partial_prefetch_failure_publishes_committed_progress() {
    let generation = AtomicU64::new(7);
    let dirty = AtomicBool::new(false);

    let result = settle_prefetch(
        Err(PrefetchError {
            message: "later route timed out".to_string(),
            changed: true,
        }),
        &generation,
        &dirty,
    );

    assert_eq!(result, Err("later route timed out".to_string()));
    assert_eq!(generation.load(Ordering::Acquire), 8);
    assert!(dirty.load(Ordering::Acquire));
}

/// Removing the permanent-miss join from `worker.rs:reconciliation_batch` would rescan every
/// unavailable historical trade on each restart instead of treating the persistent miss as done.
#[test]
fn reconciliation_skips_rows_with_a_cached_permanent_miss() {
    let _health = super::super::test_health_guard();
    let dir = std::env::temp_dir().join(format!(
        "moonterminal-reconcile-miss-{}-{}",
        std::process::id(),
        crate::util::now_unix_ms_i64()
    ));
    std::fs::create_dir_all(&dir).expect("create reconciliation fixture directory");
    let path = dir.join("valuation.sqlite");
    let minute = 1_700_000_040;
    {
        let store = super::super::open_store(&path).expect("open valuation fixture");
        super::super::store_permanent_missing(&store, 8, minute, minute * 1_000 + 120_000)
            .expect("cache permanent miss");
    }
    let reports = Connection::open_in_memory().expect("open report fixture");
    reports
        .execute_batch(
            "CREATE TABLE orders_rep (
                 core_uid INTEGER, newrecid INTEGER, closedate INTEGER,
                 basecurrency INTEGER, profitbtc REAL, spentbtc REAL
             );
             INSERT INTO orders_rep VALUES (1, 10, 1700000045, 8, 5.0, 100.0);",
        )
        .expect("seed report fixture");
    let attach = format!(
        "ATTACH DATABASE '{}' AS valuation",
        path.to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "''")
    );
    reports
        .execute(&attach, [])
        .expect("attach valuation fixture");

    let pending = reconciliation_batch(&reports, TradeSource::Typed, None, 256)
        .expect("scan startup reconciliation");

    assert!(pending.is_empty());
    drop(reports);
    std::fs::remove_dir_all(&dir).expect("remove reconciliation fixture directory");
}

/// Adding an empty-outbox early return to `worker.rs:reconciliation_batch` would strand historical
/// rows whose events were acknowledged before cache quarantine; reconciliation must discover them
/// independently of durable change notifications.
#[test]
fn empty_replacement_reconciles_rows_after_outbox_acknowledgement() {
    let _health = super::super::test_health_guard();
    let dir = std::env::temp_dir().join(format!(
        "moonterminal-reconcile-acked-{}-{}",
        std::process::id(),
        crate::util::now_unix_ms_i64()
    ));
    std::fs::create_dir_all(&dir).expect("create acknowledged reconciliation fixture");
    let path = dir.join("valuation.sqlite");
    drop(super::super::open_store(&path).expect("open empty replacement cache"));
    let reports = Connection::open_in_memory().expect("open acknowledged report fixture");
    reports
        .execute_batch(
            "CREATE TABLE orders_rep (
                 core_uid INTEGER, newrecid INTEGER, closedate INTEGER,
                 basecurrency INTEGER, profitbtc REAL, spentbtc REAL
             );
             INSERT INTO orders_rep VALUES (4, 99, 1700000045, 8, 5.0, 100.0);",
        )
        .expect("seed historical report row");
    super::super::init_report_outbox(&reports).expect("initialize empty report outbox");
    assert!(super::super::read_outbox(&reports, 10)
        .expect("read acknowledged outbox")
        .is_empty());
    let attach = format!(
        "ATTACH DATABASE '{}' AS valuation",
        path.to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "''")
    );
    reports
        .execute(&attach, [])
        .expect("attach empty replacement");

    let pending = reconciliation_batch(&reports, TradeSource::Typed, None, 256)
        .expect("scan historical rows independently of outbox");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].core_uid, 4);
    assert_eq!(pending[0].row_id, 99);
    assert_eq!(pending[0].quote_ordinal, 8);

    drop(reports);
    std::fs::remove_dir_all(&dir).expect("remove acknowledged reconciliation fixture");
}

/// Changing `worker.rs:reset_after_recovery` to leave reconciliation as `None` or retain the old
/// in-flight ACK would omit acknowledged historical trades or skip work tied to the retired cache.
#[test]
fn recovery_resets_the_full_reconciliation_cursor() {
    let input = TradeInput {
        source: TradeSource::Typed,
        core_uid: 4,
        row_id: 99,
        closedate: 1_700_000_045,
        quote_ordinal: 8,
        profit_quote: 5.0,
        spent_quote: Some(100.0),
    };
    let mut deferred = BTreeMap::from([(trade_key(&input), input)]);
    let mut reconciliation = None;
    let mut pending_ack = Some(512);

    reset_after_recovery(&mut deferred, &mut reconciliation, &mut pending_ack);

    assert!(deferred.is_empty());
    let state = reconciliation.expect("full reconciliation must restart");
    assert_eq!(state.source_index, 0);
    assert_eq!(state.after, None);
    assert_eq!(pending_ack, None);
}

/// Changing `worker.rs:unacknowledged_events` to return the full batch would process and enqueue
/// the same 512-event prefix repeatedly while the report writer is still applying its first ACK.
#[test]
fn in_flight_ack_filters_the_same_durable_prefix() {
    let events = (1..=512)
        .map(|seq| OutboxEvent {
            seq,
            source: TradeSource::Typed,
            core_uid: 1,
            row_id: seq,
            action: OutboxAction::Delete,
        })
        .collect::<Vec<_>>();
    let mut pending_ack = Some(512);

    assert!(unacknowledged_events(&events, &mut pending_ack).is_empty());
    assert_eq!(pending_ack, Some(512));

    let next = [OutboxEvent {
        seq: 513,
        source: TradeSource::Typed,
        core_uid: 1,
        row_id: 513,
        action: OutboxAction::Delete,
    }];
    assert_eq!(unacknowledged_events(&next, &mut pending_ack), &next);
    assert_eq!(pending_ack, None);
}
