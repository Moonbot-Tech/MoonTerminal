//! Valuation worker invalidation regression tests.

use super::*;
use crate::db::valuation::{RateOrientation, RatePriceBasis, ResolvedRate};

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

/// Build a status sink over fresh handles, mirroring what `spawn_worker_with_source` publishes.
///
/// Returns:
///     The sink plus the health slot, revision counter, and wake edge a `ValuationHandle` exposes.
fn status_sink() -> (
    StatusSink,
    Arc<RwLock<ValuationStatus>>,
    Arc<AtomicU64>,
    Arc<AtomicBool>,
) {
    let status = Arc::new(RwLock::new(ValuationStatus::default()));
    let revision = Arc::new(AtomicU64::new(0));
    let dirty = Arc::new(AtomicBool::new(false));
    let sink = StatusSink {
        status: status.clone(),
        revision: revision.clone(),
        dirty: dirty.clone(),
        published: ValuationStatus::default().signature(0),
    };
    (sink, status, revision, dirty)
}

/// A failing run must reach the handle, and a retry that changes nothing must not wake the UI.
///
/// Breakage: `worker.rs:note_failure` bumping `sink.revision` unconditionally instead of through
/// `StatusSink::publish`'s signature comparison. A permanently unreachable provider retries for as
/// long as the terminal runs, so the report panel and the Analytics window would take a fresh
/// status snapshot and repaint every 30 seconds forever for a chip whose text never changes.
#[test]
fn a_failing_run_publishes_once_per_transition_and_not_per_retry() {
    let (mut sink, status, revision, dirty) = status_sink();
    let mut health = ValuationStatus::default();
    let start = 1_700_000_000_000;

    note_failure(
        &mut health,
        &mut sink,
        FaultCause::new(FailureKind::Provider, "binance HTTP 429").at(ValuationStage::Reconcile),
        start,
    );
    assert_eq!(revision.load(Ordering::Acquire), 1);
    assert!(dirty.load(Ordering::Acquire));
    let published = status.read().expect("read published health").clone();
    let fault = published
        .stage(ValuationStage::Reconcile)
        .fault
        .as_ref()
        .expect("the failing stage keeps its fault")
        .clone();
    assert_eq!(fault.stage, ValuationStage::Reconcile);
    assert_eq!(fault.kind.code(), "provider");

    note_failure(
        &mut health,
        &mut sink,
        FaultCause::new(FailureKind::Provider, "binance HTTP 429").at(ValuationStage::Reconcile),
        start + 30_000,
    );
    assert_eq!(
        revision.load(Ordering::Acquire),
        1,
        "a retry repeating the same cause is not a transition"
    );
}

/// Progress on one stage must publish, and must leave another stage's failing run untouched.
///
/// Breakage: `worker.rs:run_worker` calling `record_progress` for a fixed stage rather than the one
/// whose turn just completed. The deferred-minute stage is marked healthy whenever nothing is due,
/// so stamping its progress onto `Reconcile` would repeatedly clear a permanently stuck
/// reconciliation and the footer could never report it.
#[test]
fn progress_publishes_and_is_scoped_to_its_own_stage() {
    let (mut sink, status, revision, _dirty) = status_sink();
    let mut health = ValuationStatus::default();
    let start = 1_700_000_000_000;

    for attempt in 0..3 {
        note_failure(
            &mut health,
            &mut sink,
            FaultCause::new(FailureKind::CacheWrite, "disk is full").at(ValuationStage::Reconcile),
            start + attempt * 100_000,
        );
    }
    let stalled_at = start + 300_000;
    assert!(health.stalled(stalled_at).is_some());

    record_progress(
        &mut health,
        &mut sink,
        ValuationStage::DeferredMinute,
        stalled_at,
    );
    assert!(
        status
            .read()
            .expect("read published health")
            .stalled(stalled_at)
            .is_some(),
        "an unrelated healthy stage must not clear the published stall"
    );

    let before = revision.load(Ordering::Acquire);
    record_progress(
        &mut health,
        &mut sink,
        ValuationStage::Reconcile,
        stalled_at,
    );
    assert!(revision.load(Ordering::Acquire) > before);
    assert!(!status.read().expect("read published health").is_retrying());
}

/// A turn the stage could not act on must not clear an unresolved failing run.
///
/// Breakage: `worker.rs:attempt` calling `record_progress` for every `Ok` instead of gating on
/// `StageTurn::is_progress`. `reports.sqlite` being absent is a healthy startup state that the
/// reconcile or outbox stage can report, so a provider outage the user is already being warned
/// about would be silently retracted when either stage found the replica missing — and the retry
/// counter would restart from one when it became available.
#[test]
fn a_turn_with_nothing_to_do_leaves_an_open_failing_run_alone() {
    let (mut sink, status, revision, _dirty) = status_sink();
    let mut health = ValuationStatus::default();
    let now = 1_700_000_000_000;
    let mut retry_after = None;

    note_failure(
        &mut health,
        &mut sink,
        FaultCause::new(FailureKind::Provider, "binance HTTP 503").at(ValuationStage::Reconcile),
        now,
    );
    let published = revision.load(Ordering::Acquire);

    // Far enough past the backoff that the stage is eligible, so the gate cannot mask the result.
    let later = now + 60_000;
    let outcome = attempt(
        &mut health,
        &mut sink,
        ValuationStage::Reconcile,
        later,
        &mut retry_after,
        || Ok(StageTurn::AwaitingReplica),
    );

    assert!(matches!(outcome, Attempt::Done(StageTurn::AwaitingReplica)));
    assert_eq!(
        health.stage(ValuationStage::Reconcile).consecutive_failures,
        1,
        "an absent replica says nothing about the provider that failed"
    );
    assert_eq!(
        retry_after,
        Some(REPLICA_POLL),
        "it still has to wait, or the loop spins on the missing replica"
    );
    assert_eq!(
        revision.load(Ordering::Acquire),
        published,
        "and nothing new was published"
    );
    assert!(
        status
            .read()
            .expect("read published health")
            .stage(ValuationStage::Reconcile)
            .fault
            .is_some(),
        "the published fault is still the provider one"
    );
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
        resolved_minute_utc: minute,
        rate_usdt: 0.999,
        provider: "fixture".to_string(),
        symbol: "USDCUSDT".to_string(),
        orientation: RateOrientation::Direct,
        price_basis: RatePriceBasis::ExactClose,
        candle_open_ms: minute * 1_000,
        candle_close_ms: minute * 1_000 + 59_999,
        leg2_provider: None,
        leg2_symbol: None,
        leg2_orientation: None,
        leg1_rate: 0.999,
        leg2_rate: None,
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
        &BTreeSet::new(),
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
        &BTreeSet::new(),
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
        &BTreeSet::new(),
    );

    assert!(matches!(deleted, PrepareResult::Complete { changed: true }));
    let remaining = store
        .query_row("SELECT row_id FROM trade_values", [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("read surviving identity");
    assert_eq!(remaining, 11);
}

/// Breakage: retaining an older deferred copy when a row event becomes immediately valueable lets
/// the stale input retry later and overwrite the current prepared value.
#[test]
fn row_event_replaces_a_stale_deferred_copy() {
    let store =
        super::super::open_store(std::path::Path::new(":memory:")).expect("open valuation fixture");
    let reports = Connection::open_in_memory().expect("open report fixture");
    let closedate = current_minute_utc() - 60;
    reports
        .execute_batch(&format!(
            "CREATE TABLE orders_rep (
                 core_uid INTEGER, newrecid INTEGER, closedate INTEGER,
                 basecurrency INTEGER, profitbtc REAL, spentbtc REAL
             );
             INSERT INTO orders_rep VALUES (1, 10, {closedate}, 1, 7.0, 3.0);"
        ))
        .expect("seed current report row");
    let stale = TradeInput {
        source: TradeSource::Typed,
        core_uid: 1,
        row_id: 10,
        closedate: closedate - 600,
        quote_ordinal: 8,
        profit_quote: 999.0,
        spent_quote: None,
    };
    let key = trade_key(&stale);
    let mut deferred = BTreeMap::from([(key, stale)]);

    let result = process_event(
        &store,
        &NoNetwork,
        &reports,
        OutboxEvent {
            seq: 1,
            source: TradeSource::Typed,
            core_uid: 1,
            row_id: 10,
            action: OutboxAction::Row,
        },
        &mut deferred,
        &BTreeSet::new(),
    );

    assert!(matches!(result, PrepareResult::Complete { changed: true }));
    assert!(deferred.is_empty());
    assert_eq!(
        store
            .query_row(
                "SELECT closedate, quote_ordinal, profit_quote FROM trade_values",
                [],
                |row| Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?
                )),
            )
            .expect("read current prepared value"),
        (closedate, 1, 7.0)
    );
}

/// Breakage: removing `DEFERRED_BATCH` lets a large restored history monopolize one worker turn,
/// delaying newly committed report events until every old row has been retried.
#[test]
fn deferred_processing_yields_after_one_bounded_batch() {
    let store =
        super::super::open_store(std::path::Path::new(":memory:")).expect("open valuation fixture");
    let minute = current_minute_utc() - 60;
    let mut deferred = (0..=DEFERRED_BATCH)
        .map(|offset| {
            let input = TradeInput {
                source: TradeSource::Typed,
                core_uid: 1,
                row_id: offset as i64,
                closedate: minute,
                quote_ordinal: 1,
                profit_quote: 1.0,
                spent_quote: None,
            };
            (trade_key(&input), input)
        })
        .collect::<BTreeMap<_, _>>();
    let generation = AtomicU64::new(0);
    let dirty = AtomicBool::new(false);

    let turn = process_deferred(&store, &NoNetwork, &generation, &dirty, &mut deferred)
        .expect("process one deferred batch");

    assert!(matches!(turn, StageTurn::Ran { more: true }));
    assert_eq!(deferred.len(), 1);
}

/// A provider range with no candle must persist retry pacing without creating a terminal rate.
///
/// Breakage: restoring a permanent-miss row makes the trade disappear from pending coverage and
/// prevents the later market observation from ever entering the user's historical total.
#[test]
fn unresolved_rate_remains_retryable_without_a_terminal_cache_entry() {
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

    let result = prepare_trade(&store, &MissingSource, &input, false);
    assert!(matches!(result, PrepareResult::Deferred { changed: false }));
    assert_eq!(
        super::super::cached_rate(&store, 8, minute).expect("read unresolved rate"),
        None
    );
    assert!(
        super::super::rate_search_start(&store, 8, minute, now_unix_ms_i64())
            .expect("read retry schedule")
            .is_none()
    );
}

/// Breakage: dropping the canonical-exact miss set from `prefetch_rates` makes per-row preparation
/// repeat the same Binance/Bybit exact requests that the batch already proved empty.
#[test]
fn exact_prefetch_misses_are_not_requested_again_per_row() {
    let store =
        super::super::open_store(std::path::Path::new(":memory:")).expect("open valuation fixture");
    let minute = current_minute_utc() - 120;
    let input = TradeInput {
        source: TradeSource::Typed,
        core_uid: 1,
        row_id: 10,
        closedate: minute,
        quote_ordinal: 8,
        profit_quote: 5.0,
        spent_quote: None,
    };
    let source = CountingSource::new(&[]);

    let prefetched = prefetch_rates(&store, &source, std::slice::from_ref(&input))
        .expect("prefetch exact routes");
    assert!(prefetched
        .canonical_exact_missing
        .contains(&(input.quote_ordinal, minute)));
    source.calls.lock().expect("clear prefetch calls").clear();

    assert!(matches!(
        prepare_trade(&store, &source, &input, true),
        PrepareResult::Deferred { .. }
    ));
    assert!(source
        .calls
        .lock()
        .expect("read preparation calls")
        .iter()
        .all(|(provider, symbol, start, end)| {
            !matches!(*provider, "binance_spot" | "bybit_spot")
                || !matches!(symbol.as_str(), "USDCUSDT" | "USDTUSDC")
                || *start != minute
                || *end != minute
        }));
}

/// A persisted retry that becomes due must consume a newly retained successor without any new
/// report event. Breakage: dropping the deferred row after acknowledging its outbox event leaves
/// the user's historical total pending forever on a long-running server.
#[test]
fn due_retry_values_the_row_when_a_successor_appears() {
    let store =
        super::super::open_store(std::path::Path::new(":memory:")).expect("open valuation fixture");
    let requested = current_minute_utc() - 180;
    let successor = requested + 60;
    let input = TradeInput {
        source: TradeSource::Typed,
        core_uid: 1,
        row_id: 10,
        closedate: requested + 5,
        quote_ordinal: 8,
        profit_quote: 5.0,
        spent_quote: Some(100.0),
    };
    assert!(matches!(
        prepare_trade(&store, &MissingSource, &input, false),
        PrepareResult::Deferred { changed: false }
    ));
    store
        .execute(
            "UPDATE rate_searches SET searched_through_minute=?1, next_retry_at_ms=0",
            [requested],
        )
        .expect("make the next minute newly searchable");
    let source = CountingSource::new(&[("USDCUSDT", 1.001)]).at_minute(successor);
    let generation = AtomicU64::new(0);
    let dirty = AtomicBool::new(false);
    let key = trade_key(&input);
    let mut deferred = BTreeMap::from([(key, input)]);

    process_deferred(&store, &source, &generation, &dirty, &mut deferred)
        .expect("process due successor");

    assert!(deferred.is_empty());
    let stored = store
        .query_row(
            "SELECT resolved_minute_utc, price_basis FROM rates WHERE quote_ordinal=8",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .expect("read successor provenance");
    assert_eq!(stored, (successor, RatePriceBasis::SuccessorOpen.code()));
    assert_eq!(
        store
            .query_row("SELECT COUNT(*) FROM trade_values", [], |row| row
                .get::<_, i64>(0))
            .expect("count prepared rows"),
        1
    );
}

/// Removing the error-side publication from `worker.rs:settle_prefetch` would leave earlier
/// prepared rows invisible while a later provider retry keeps failing.
#[test]
fn partial_prefetch_failure_publishes_committed_progress() {
    let generation = AtomicU64::new(7);
    let dirty = AtomicBool::new(false);

    let result = settle_prefetch(
        Err(PrefetchError {
            fault: FaultCause::new(FailureKind::Provider, "later route timed out"),
            changed: true,
        }),
        &generation,
        &dirty,
    );

    assert_eq!(
        result,
        Err(FaultCause::new(
            FailureKind::Provider,
            "later route timed out"
        ))
    );
    assert_eq!(generation.load(Ordering::Acquire), 8);
    assert!(dirty.load(Ordering::Acquire));
}

/// Omitting unresolved rows from startup reconciliation loses their in-memory wake after restart;
/// ignoring the persisted boundary then turns the restored row into a provider hot loop.
#[test]
fn reconciliation_restores_pending_rows_without_bypassing_the_retry_boundary() {
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
        super::super::store_rate_search(&store, 8, minute, minute + 60, now_unix_ms_i64())
            .expect("persist retry boundary");
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

    assert_eq!(pending.len(), 1);
    let store = super::super::open_store(&path).expect("reopen valuation fixture");
    let deferred = BTreeMap::from([(trade_key(&pending[0]), pending[0].clone())]);
    assert!(!current_minute_closed_any(&store, &deferred));
    drop(store);
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

/// Minimal attached report fixture for reconciliation-walk tests.
///
/// Args:
///     rows: `(core_uid, newrecid, closedate)` tuples inserted in the given order.
///
/// Returns:
///     Fixture directory and an in-memory report connection with an attached valuation cache.
fn report_fixture(rows: &[(i64, i64, i64)]) -> (std::path::PathBuf, Connection) {
    let dir = std::env::temp_dir().join(format!(
        "moonterminal-reconcile-order-{}-{}",
        std::process::id(),
        crate::util::now_unix_ms_i64()
    ));
    std::fs::create_dir_all(&dir).expect("create reconciliation order fixture directory");
    let path = dir.join("valuation.sqlite");
    drop(super::super::open_store(&path).expect("open valuation fixture"));
    let reports = Connection::open_in_memory().expect("open report fixture");
    reports
        .execute_batch(
            "CREATE TABLE orders_rep (
                 core_uid INTEGER, newrecid INTEGER, closedate INTEGER,
                 basecurrency INTEGER, profitbtc REAL, spentbtc REAL
             );",
        )
        .expect("create report fixture");
    for (core_uid, row_id, closedate) in rows {
        reports
            .execute(
                "INSERT INTO orders_rep VALUES (?1, ?2, ?3, 1, 1.0, 10.0)",
                rusqlite::params![core_uid, row_id, closedate],
            )
            .expect("seed report fixture row");
    }
    // Attach through the production helper so the fixture exercises the same read-only URI and
    // validation path the app uses, rather than a connection shape production never creates.
    assert!(
        super::super::attach_store(&reports, &path).expect("attach valuation fixture"),
        "fixture cache must attach"
    );
    (dir, reports)
}

/// Restoring the ascending `ORDER BY r.core_uid, r.{id_column}` in
/// `worker.rs:reconciliation_batch` would value the oldest trades of the lowest-numbered core
/// first, so a user watching today's report waits out the entire history before their rows carry a
/// USDT value — the 47-minute, 539k-row backfill this ordering exists to avoid.
#[test]
fn reconciliation_values_the_newest_trades_before_older_ones() {
    let _health = super::super::test_health_guard();
    // Deliberately adversarial: the NEWEST trade belongs to the HIGHEST core uid, so an ascending
    // core-ordered walk would return it last rather than first.
    let (dir, reports) = report_fixture(&[
        (1, 10, 1_700_000_060),
        (1, 11, 1_700_000_120),
        (22, 100, 1_700_009_000),
        (22, 101, 1_700_008_000),
        (7, 55, 1_700_005_000),
    ]);

    let batch = reconciliation_batch(&reports, TradeSource::Typed, None, 256)
        .expect("scan newest-first reconciliation");

    let order = batch
        .iter()
        .map(|input| (input.core_uid, input.row_id))
        .collect::<Vec<_>>();
    assert_eq!(
        order,
        vec![(22, 100), (22, 101), (7, 55), (1, 11), (1, 10)],
        "batch must descend by closedate regardless of core uid"
    );
    drop(reports);
    std::fs::remove_dir_all(&dir).expect("remove reconciliation order fixture directory");
}

/// Dropping `core_uid`/`row_id` from the descending keyset in `worker.rs:reconciliation_batch`
/// would leave the cursor unable to separate rows that share a `closedate`: the walk either
/// re-reads a tied group forever or steps over its remainder. Both are silent — the first spins the
/// worker, the second permanently leaves those trades unvalued.
#[test]
fn reconciliation_visits_every_row_once_when_close_dates_tie() {
    let _health = super::super::test_health_guard();
    // Five rows share one closedate across three cores; two more sit on either side of it.
    let seeded = [
        (1, 10, 1_700_000_500),
        (3, 20, 1_700_000_400),
        (3, 21, 1_700_000_400),
        (9, 30, 1_700_000_400),
        (9, 31, 1_700_000_400),
        (2, 40, 1_700_000_400),
        (5, 50, 1_700_000_300),
    ];
    let (dir, reports) = report_fixture(&seeded);

    // Batch size 2 forces the cursor to resume inside the tied group.
    let mut cursor = None;
    let mut visited = Vec::new();
    // Bounded on purpose: a lost tie-break must fail this test, never hang the suite.
    let max_turns = seeded.len() * 2 + 4;
    for turn in 0..=max_turns {
        assert!(
            turn < max_turns,
            "reconciliation cursor failed to terminate"
        );
        let batch = reconciliation_batch(&reports, TradeSource::Typed, cursor, 2)
            .expect("scan tied reconciliation batch");
        if batch.is_empty() {
            break;
        }
        cursor = batch
            .last()
            .map(|input| (input.closedate, input.core_uid, input.row_id));
        visited.extend(batch.iter().map(|input| (input.core_uid, input.row_id)));
    }

    let mut seen = visited;
    seen.sort_unstable();
    let mut expected = seeded
        .iter()
        .map(|(core_uid, row_id, _)| (*core_uid, *row_id))
        .collect::<Vec<_>>();
    expected.sort_unstable();
    // Comparing sorted vectors (not sets) catches a duplicate visit as well as a missing one.
    assert_eq!(
        seen, expected,
        "every seeded row must be visited exactly once"
    );
    drop(reports);
    std::fs::remove_dir_all(&dir).expect("remove reconciliation order fixture directory");
}

/// Lowering `worker.rs:reconciliation_batch`'s `(i64::MAX, i64::MAX, i64::MAX)` seed to any value
/// at or below a real key — the plausible shape being a leftover ascending `(-1, -1, -1)` — makes a
/// restart resume below the newest rows instead of above them. Trades that arrived while the app
/// was closed then stay unvalued until the whole remaining backlog drains, which is the very wait
/// the descending walk exists to remove.
#[test]
fn a_restart_covers_trades_that_arrived_above_the_previous_cursor() {
    let _health = super::super::test_health_guard();
    let (dir, reports) = report_fixture(&[
        (1, 10, 1_700_000_100),
        (1, 11, 1_700_000_200),
        (1, 12, 1_700_000_300),
    ]);
    // Walk down to the oldest row, as a long backfill would.
    let cursor = Some((1_700_000_200, 1, 11));
    let tail = reconciliation_batch(&reports, TradeSource::Typed, cursor, 256)
        .expect("scan reconciliation tail");
    assert_eq!(
        tail.iter().map(|i| i.row_id).collect::<Vec<_>>(),
        vec![10],
        "a descended cursor must only see older rows"
    );

    // A newer trade lands while the cursor sits in history.
    reports
        .execute(
            "INSERT INTO orders_rep VALUES (4, 99, ?1, 1, 1.0, 10.0)",
            rusqlite::params![1_700_000_900_i64],
        )
        .expect("insert a newer trade");

    let resumed =
        reconciliation_batch(&reports, TradeSource::Typed, None, 256).expect("scan after restart");

    assert_eq!(
        resumed.first().map(|input| (input.core_uid, input.row_id)),
        Some((4, 99)),
        "a restart must start at the newest row, not the previous cursor"
    );
    drop(reports);
    std::fs::remove_dir_all(&dir).expect("remove reconciliation order fixture directory");
}

/// Provider boundary that records every request and answers from a scripted price table.
struct CountingSource {
    /// Symbols that resolve, and the close they resolve to.
    prices: std::collections::HashMap<&'static str, f64>,
    /// Every `(provider, symbol, start minute, end minute)` asked for, in request order.
    calls: Mutex<Vec<(&'static str, String, i64, i64)>>,
    /// When set, every request fails transiently with this reason instead of answering.
    transient: Option<&'static str>,
    /// Optional fixed candle minute used to model a sparse provider response.
    candle_minute: Option<i64>,
}

impl CountingSource {
    /// Build a source answering only the listed symbols.
    ///
    /// Args:
    ///     prices: Provider symbols and the closes they should return.
    ///
    /// Returns:
    ///     A scripted source that treats every unlisted route as permanently absent.
    fn new(prices: &[(&'static str, f64)]) -> Self {
        Self {
            prices: prices.iter().copied().collect(),
            calls: Mutex::new(Vec::new()),
            transient: None,
            candle_minute: None,
        }
    }

    /// Build a source whose every route fails transiently.
    ///
    /// Returns:
    ///     A scripted source that reports a connection failure for every request.
    fn unreachable() -> Self {
        Self {
            prices: std::collections::HashMap::new(),
            calls: Mutex::new(Vec::new()),
            transient: Some("connection reset"),
            candle_minute: None,
        }
    }

    /// Pin every scripted candle to one provider minute.
    ///
    /// Args:
    ///     minute: Fixed UTC candle-open minute in Unix seconds.
    ///
    /// Returns:
    ///     This source configured to return the candle only when the request contains that minute.
    fn at_minute(mut self, minute: i64) -> Self {
        self.candle_minute = Some(minute);
        self
    }

    /// How many provider requests have been made so far.
    ///
    /// Returns:
    ///     The number of calls recorded across all routes.
    fn call_count(&self) -> usize {
        self.calls.lock().expect("call log").len()
    }
}

impl SpotRateSource for CountingSource {
    /// Answer a scripted symbol with one closed candle, or report the route absent.
    ///
    /// Args:
    ///     provider: Canonical provider identifier.
    ///     symbol: Provider-native market symbol.
    ///     start_minute_utc: First requested UTC minute.
    ///     end_minute_utc: Last requested UTC minute.
    ///
    /// Returns:
    ///     One candle for a scripted symbol, or a route/range or transient failure.
    fn candles(
        &self,
        provider: &'static str,
        symbol: &str,
        start_minute_utc: i64,
        end_minute_utc: i64,
    ) -> Result<Vec<super::super::provider::SpotCandle>, FetchFailure> {
        self.calls.lock().expect("call log").push((
            provider,
            symbol.to_string(),
            start_minute_utc,
            end_minute_utc,
        ));
        if let Some(reason) = self.transient {
            return Err(FetchFailure::Transient(reason.to_string()));
        }
        let candle_minute = self.candle_minute.unwrap_or(start_minute_utc);
        if !(start_minute_utc..=end_minute_utc).contains(&candle_minute) {
            return Err(FetchFailure::Missing);
        }
        match self.prices.get(symbol) {
            Some(&close) => Ok(vec![super::super::provider::SpotCandle {
                open_ms: candle_minute * 1000,
                close_ms: candle_minute * 1000 + 59_999,
                open: close,
                close,
            }]),
            None => Err(FetchFailure::Missing),
        }
    }
}

/// The minute every current-rate fixture resolves against.
const RATE_MINUTE: i64 = 1_800_000_000;

/// Serializes the tests that publish a current-rate snapshot.
///
/// `publish_current_rates` replaces ONE process-wide cell, so tests that publish run against
/// shared state whatever their own fixtures say. Only a test that READS that cell back strictly
/// needs the lock, but a writer running unserialized beside it would overwrite the value under
/// assertion — so every publisher takes it.
static PUBLISH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Claim the publication lock, ignoring a poisoning left by an unrelated failed test.
fn publish_guard() -> std::sync::MutexGuard<'static, ()> {
    PUBLISH_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Build a pass already scoped to the given ordinals, as if the report scan had just run.
///
/// Args:
///     ordinals: Quote ordinals queued for the pass.
///
/// Returns:
///     Current-rate state scoped to [`RATE_MINUTE`].
fn scoped_pass(ordinals: &[i64]) -> CurrentRateState {
    CurrentRateState {
        minute: Some(RATE_MINUTE),
        pending: ordinals.to_vec(),
        scanned: Some(RATE_MINUTE),
        ordinals: ordinals.to_vec(),
        ..CurrentRateState::default()
    }
}

/// The refresh throttle must be read in the unit its operands actually carry.
///
/// Breakage: `worker.rs::refresh_is_due` comparing `minute - armed` against
/// `CURRENT_REFRESH_MINUTES` without the `* 60`. `current_minute_utc` returns Unix SECONDS rounded
/// to a minute, so adjacent minutes differ by 60 and `60 >= 5` holds immediately — the five-minute
/// throttle silently becomes a one-minute one, and both the provider traffic and the requery of
/// every open Report host and the Analytics window go back to once a minute. It reads correct at a
/// glance, which is exactly why it needs pinning; the scan throttle three lines below carries the
/// `* 60` and would keep working, so nothing else would flag it.
#[test]
fn the_refresh_throttle_counts_seconds_not_minutes() {
    let armed = RATE_MINUTE;
    assert!(
        refresh_is_due(armed, None, true),
        "a pass that has never run is due"
    );
    assert!(
        !refresh_is_due(armed + 60, Some(armed), true),
        "one minute later is NOT due"
    );
    assert!(
        !refresh_is_due(armed + CURRENT_REFRESH_MINUTES * 60 - 1, Some(armed), true),
        "one second short of the interval is not due"
    );
    assert!(
        refresh_is_due(armed + CURRENT_REFRESH_MINUTES * 60, Some(armed), true),
        "the interval itself is due"
    );
    assert!(
        !refresh_is_due(armed + 3_600, Some(armed), false),
        "a pass still draining is never re-armed under itself"
    );
}

/// A cold refresh of K currencies must cost K turns, not one turn of K sequential network waits.
///
/// Breakage: `worker.rs::resolve_next_rate` draining `state.pending` in a loop instead of taking
/// one entry per call. Each currency costs up to four sequential provider routes at a fifteen
/// second timeout, so a batched turn would park reconciliation and the outbox behind minutes of
/// network wait while the user watches an unmoving backfill.
#[test]
fn a_refresh_pass_asks_one_provider_per_turn() {
    let _published = publish_guard();
    let source = CountingSource::new(&[("BTCUSDT", 60_000.0), ("ETHUSDT", 3_000.0)]);
    let generation = AtomicU64::new(0);
    let dirty = AtomicBool::new(false);
    let mut state = scoped_pass(&[0, 2]);

    let first = resolve_next_rate(&source, &generation, &dirty, &mut state, RATE_MINUTE);
    assert_eq!(first, Ok(StageTurn::Ran { more: true }));
    assert_eq!(source.call_count(), 1, "one currency per turn");

    let second = resolve_next_rate(&source, &generation, &dirty, &mut state, RATE_MINUTE);
    assert_eq!(second, Ok(StageTurn::Drained));
    assert_eq!(source.call_count(), 2);
    assert_eq!(
        generation.load(Ordering::Relaxed),
        1,
        "the finished pass publishes exactly once, through the DATA generation"
    );
    assert!(dirty.load(Ordering::Relaxed));
}

/// Breakage: changing `worker.rs::resolve_next_rate` back to an exact `minute - 60` request would
/// leave a sparse USDC rate unavailable, preventing Analytics from aggregating current PnL even
/// though a closed candle still exists inside freshness.
#[test]
fn a_current_refresh_requests_the_full_closed_freshness_window() {
    let _published = publish_guard();
    let sparse_minute = RATE_MINUTE - 120;
    let source = CountingSource::new(&[("USDCUSDT", 1.001)]).at_minute(sparse_minute);
    let generation = AtomicU64::new(0);
    let dirty = AtomicBool::new(false);
    let mut state = scoped_pass(&[8]);

    assert_eq!(
        resolve_next_rate(&source, &generation, &dirty, &mut state, RATE_MINUTE),
        Ok(StageTurn::Drained)
    );
    assert_eq!(state.rates.get(&8).map(|rate| rate.rate_usdt), Some(1.001));
    assert_eq!(
        source.calls.lock().expect("call log").as_slice(),
        &[(
            "binance_spot",
            "USDCUSDT".to_string(),
            RATE_MINUTE - super::super::FRESHNESS_MS / 1_000,
            RATE_MINUTE - 60,
        )],
    );
}

/// A drained pass must issue no further requests until the refresh gate arms another one.
///
/// Breakage: `worker.rs::resolve_next_rate` refilling `pending` when it empties. The worker
/// re-runs every stage each 25 ms while reconciling, so that would hammer the exchange
/// continuously and get the user's address rate-limited.
///
/// The refresh gate in `refresh_current_rates` that decides WHEN `pending` is refilled is a
/// separate guard, and this test does not reach it: it calls `resolve_next_rate` directly, because
/// the enclosing function opens a report reader against the real data directory.
#[test]
fn a_drained_pass_stops_asking_within_the_same_minute() {
    let _published = publish_guard();
    let source = CountingSource::new(&[("BTCUSDT", 60_000.0)]);
    let generation = AtomicU64::new(0);
    let dirty = AtomicBool::new(false);
    let mut state = scoped_pass(&[0]);

    assert_eq!(
        resolve_next_rate(&source, &generation, &dirty, &mut state, RATE_MINUTE),
        Ok(StageTurn::Drained)
    );
    let after_first_pass = source.call_count();
    for _ in 0..5 {
        assert_eq!(
            resolve_next_rate(&source, &generation, &dirty, &mut state, RATE_MINUTE),
            Ok(StageTurn::Drained)
        );
    }
    assert_eq!(
        source.call_count(),
        after_first_pass,
        "a drained pass must cost nothing"
    );
}

/// A currency whose routes have all gone permanently absent must lose its cached price.
///
/// Breakage: `worker.rs::resolve_next_rate` inserting into `missing` without removing from
/// `rates` — a delisted market's last price would keep rendering as the current rate forever,
/// because nothing else would ever refresh it away.
#[test]
fn a_permanently_missing_route_drops_the_price_it_used_to_have() {
    let _published = publish_guard();
    let generation = AtomicU64::new(0);
    let dirty = AtomicBool::new(false);
    let mut state = scoped_pass(&[0]);
    let listed = CountingSource::new(&[("BTCUSDT", 60_000.0)]);
    resolve_next_rate(&listed, &generation, &dirty, &mut state, RATE_MINUTE).expect("first pass");
    assert!(state.rates.contains_key(&0));

    state.pending = vec![0];
    let delisted = CountingSource::new(&[]);
    resolve_next_rate(&delisted, &generation, &dirty, &mut state, RATE_MINUTE)
        .expect("second pass");
    assert!(
        !state.rates.contains_key(&0),
        "the stale price must be gone"
    );
    assert!(state.missing.contains(&0));
}

/// A transient provider failure must leave the currency queued rather than silently skipped.
///
/// Breakage: `worker.rs::resolve_next_rate` popping `pending` before the request rather than after
/// it resolves — one connection reset would drop that currency until a later refresh pass, and its
/// trades would read as permanently unconvertible rather than as still being fetched.
#[test]
fn a_transient_failure_leaves_the_currency_queued() {
    let source = CountingSource::unreachable();
    let generation = AtomicU64::new(0);
    let dirty = AtomicBool::new(false);
    let mut state = scoped_pass(&[0, 2]);

    let outcome = resolve_next_rate(&source, &generation, &dirty, &mut state, RATE_MINUTE);
    assert!(
        outcome.is_err(),
        "a transient failure is reported, not swallowed"
    );
    assert_eq!(state.pending, vec![0, 2], "nothing is consumed");
    assert!(state.rates.is_empty());
    assert!(state.missing.is_empty(), "transient is not permanent");
    assert_eq!(
        generation.load(Ordering::Relaxed),
        0,
        "nothing was published"
    );
}

/// A rate that ages past the window must be dropped even when nothing new can be fetched.
///
/// Breakage: `worker.rs::refresh_current_rates` checking freshness only on the success path, or
/// dropping the `CurrentRateState::expire_stale` call before the provider request. Freshness is evaluated when
/// SQL is built, and SQL is only rebuilt when the data generation moves — so during a provider
/// outage longer than the window nothing would ever move it, and an expired rate would keep
/// rendering under a label that calls it current.
#[test]
fn an_expired_rate_is_dropped_even_while_the_provider_is_unreachable() {
    let mut state = scoped_pass(&[0, 2]);
    let fresh_at = RATE_MINUTE * 1000;
    for ordinal in [0, 2] {
        state.rates.insert(
            ordinal,
            super::super::CurrentRate {
                rate_usdt: 1.0,
                provider: "binance_spot".to_string(),
                symbol: "X".to_string(),
                fetched_at_ms: fresh_at,
            },
        );
    }

    assert!(
        !state.expire_stale(fresh_at + super::super::FRESHNESS_MS - 1),
        "one millisecond inside the window keeps both"
    );
    assert_eq!(state.rates.len(), 2);

    assert!(
        state.expire_stale(fresh_at + super::super::FRESHNESS_MS),
        "reaching the window edge must report a change so the caller republishes"
    );
    assert!(state.rates.is_empty());
}

/// The wake deadline must be the EARLIEST expiry, not the latest.
///
/// Breakage: `worker.rs::CurrentRateState::next_expiry_ms` using `.max()` instead of `.min()`, or
/// the park cap in `run_worker` reading it without one. The worker evaluates expiry when its loop
/// turns, so a deadline taken from the freshest rate would let the stalest one sit on screen past
/// the cutoff for as long as the difference between them — and during a provider outage the park
/// is a five-minute backoff, so nothing else would wake it.
#[test]
fn the_wake_deadline_follows_the_rate_that_expires_first() {
    let mut state = scoped_pass(&[0, 2]);
    assert_eq!(
        state.next_expiry_ms(),
        None,
        "nothing held, nothing to wake for"
    );

    let base = RATE_MINUTE * 1000;
    for (ordinal, fetched_at_ms) in [(0, base + 90_000), (2, base)] {
        state.rates.insert(
            ordinal,
            super::super::CurrentRate {
                rate_usdt: 1.0,
                provider: "binance_spot".to_string(),
                symbol: "X".to_string(),
                fetched_at_ms,
            },
        );
    }

    assert_eq!(
        state.next_expiry_ms(),
        Some(base + super::super::FRESHNESS_MS),
        "the oldest rate sets the deadline"
    );
}

/// The scan must return the currencies needing a rate, and only those.
///
/// Breakage: `worker.rs::report_quote_ordinals` dropping the identity-USDT exclusion or the
/// storage-class guard — every quote scan would schedule needless provider requests for USDT and
/// placeholder values that decode to no currency at all.
#[test]
fn the_quote_scan_returns_only_currencies_that_need_a_rate() {
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER NOT NULL, closedate INTEGER,
                                  basecurrency, profitbtc REAL, newrecid INTEGER);
         INSERT INTO orders_rep VALUES (1, 10, 1, 1.0, 1),
                                       (1, 11, 0, 1.0, 2),
                                       (1, 12, 8, 1.0, 3),
                                       (1, 13, 8, 1.0, 4),
                                       (1, 14, 99, 1.0, 5),
                                       (1, 15, 'x', 1.0, 6),
                                       (1, 16, NULL, 1.0, 7)",
    )
    .expect("schema");
    let ordinals = report_quote_ordinals(&conn).expect("scan");
    assert_eq!(ordinals, vec![0, 8]);
}

/// Re-fetching prices that have not moved must wake nobody, while the snapshot itself is stored.
///
/// Breakage: `worker.rs::CurrentRateState::publish_snapshot` bumping the generation
/// unconditionally, or `renders_differently` folding `fetched_at_ms` into the comparison — every
/// open Report host and the Analytics window requery on that generation and reload their whole
/// tree, so a refresh that changed no figure redraws every surface for nothing. The refresh
/// interval alone does not remove it: a pegged quote's price never moves, and its rate would still
/// be republished for as long as the mode stays on.
///
/// The mirror-image breakage is publishing NOTHING when unchanged — an `if changed` wrapped around
/// the `publish_current_rates` call as well as the generation bump. Freshness is judged against the
/// PUBLISHED snapshot, so a re-fetch that stored nothing would let the cutoff retire a rate the
/// worker had in fact just refreshed. That half is asserted through the published snapshot itself,
/// not through the private diff baseline, because the baseline is not what any reader consults.
#[test]
fn republishing_the_same_prices_costs_no_requery() {
    /// Judged from an instant where every fixture rate below is inside the freshness window, so
    /// only the PRICE comparison can decide the bump here.
    const NOW: i64 = 600_000;

    let _published = publish_guard();
    let generation = AtomicU64::new(0);
    let dirty = AtomicBool::new(false);
    let mut state = scoped_pass(&[0]);
    let rate = |rate_usdt: f64, fetched_at_ms: i64| super::super::CurrentRate {
        rate_usdt,
        provider: "binance_spot".to_string(),
        symbol: "BTCUSDT".to_string(),
        fetched_at_ms,
    };
    // Whether the published snapshot still serves the quote at `now_ms`, which is the exact
    // question `valuation::projection` asks of it.
    let served_at = |now_ms: i64| {
        super::super::current::current_rates_at()
            .0
            .fresh(now_ms)
            .any(|(ordinal, _)| ordinal == 0)
    };

    state.rates.insert(0, rate(60_000.0, 1_000));
    state.publish_snapshot(&generation, &dirty, NOW);
    assert_eq!(
        generation.load(Ordering::Relaxed),
        1,
        "the first snapshot is new to every surface"
    );
    assert!(dirty.swap(false, Ordering::Relaxed), "a bump wakes the UI");

    state.rates.insert(0, rate(60_000.0, 400_000));
    state.publish_snapshot(&generation, &dirty, NOW);
    assert_eq!(
        generation.load(Ordering::Relaxed),
        1,
        "an unchanged price must cost no requery"
    );
    assert!(
        !dirty.load(Ordering::Relaxed),
        "withholding the bump must withhold the wake edge with it"
    );
    // The re-fetch happened at 400_000, so the quote must survive to 400_000 + FRESHNESS_MS. Had
    // the store been skipped, the published snapshot would still carry the 1_000 fetch and this
    // instant would already be past its cutoff.
    assert!(
        served_at(400_000 + super::super::FRESHNESS_MS - 1),
        "the refreshed fetch instant must reach the published snapshot"
    );
    assert!(
        !served_at(400_000 + super::super::FRESHNESS_MS),
        "and the cutoff must still be measured from it"
    );

    state.rates.insert(0, rate(61_000.0, 500_000));
    state.publish_snapshot(&generation, &dirty, NOW);
    assert_eq!(
        generation.load(Ordering::Relaxed),
        2,
        "a moved price must reach every surface"
    );
    assert!(dirty.swap(false, Ordering::Relaxed), "a bump wakes the UI");

    // An expiry shrinks the map, and that IS a visible change: the retired quote falls back to
    // uncovered. The freshness contract therefore survives the conditional bump.
    state.rates.clear();
    state.publish_snapshot(&generation, &dirty, NOW);
    assert_eq!(
        generation.load(Ordering::Relaxed),
        3,
        "an expiry must still reach the screen on schedule"
    );
}

/// A pass that outlived the freshness window must reach the screen even at an unchanged price.
///
/// Breakage: `worker.rs::CurrentRateState::renders_differently` comparing only prices, provenance
/// and the missing set. The snapshot is published once the pass DRAINS, and a pass can outlive the
/// window it refreshes: each currency permits four
/// sequential provider routes, and a transient failure adds the stage's 30-300 s backoff. In that
/// interval the previously published rate ages past the cutoff and every surface renders the quote
/// as uncovered. If the pass then lands the same price, a price-only comparison withholds the bump
/// and the screen keeps saying "uncovered" over a snapshot that covers it — until some unrelated
/// change happens along.
#[test]
fn a_refresh_that_outlived_the_window_wakes_the_surfaces_at_an_unchanged_price() {
    let _published = publish_guard();
    let generation = AtomicU64::new(0);
    let dirty = AtomicBool::new(false);
    let mut state = scoped_pass(&[0]);
    let rate = |fetched_at_ms: i64| super::super::CurrentRate {
        rate_usdt: 60_000.0,
        provider: "binance_spot".to_string(),
        symbol: "BTCUSDT".to_string(),
        fetched_at_ms,
    };

    state.rates.insert(0, rate(0));
    state.publish_snapshot(&generation, &dirty, 0);
    assert_eq!(generation.load(Ordering::Relaxed), 1);

    // The pass took longer than the window. At this instant the published rate has expired, so the
    // quote is rendering as uncovered — and the fetch that just landed carries the same price.
    let late = super::super::FRESHNESS_MS + 60_000;
    state.rates.insert(0, rate(late));
    state.publish_snapshot(&generation, &dirty, late);
    assert_eq!(
        generation.load(Ordering::Relaxed),
        2,
        "a quote re-entering the freshness window changes what renders, price or no price"
    );
}
