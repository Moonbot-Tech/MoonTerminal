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
