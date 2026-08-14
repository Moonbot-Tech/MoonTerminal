//! Persistence tests for historical rate and prepared-trade storage.

use super::*;

/// Build an isolated valuation family and quarantine layout for one filesystem test.
///
/// Args:
///     tag: Test-specific path discriminator.
///
/// Returns:
///     Root, SQLite family, damage root, and pending retirement directory.
fn recovery_fixture(tag: &str) -> (PathBuf, [PathBuf; 3], PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "moonterminal-valuation-recovery-{tag}-{}-{}",
        std::process::id(),
        crate::util::now_unix_ms_i64()
    ));
    std::fs::create_dir_all(&root).expect("create valuation recovery fixture");
    let files = [
        root.join("valuation.sqlite"),
        root.join("valuation.sqlite-wal"),
        root.join("valuation.sqlite-shm"),
    ];
    let damage_root = root.join("damaged-valuation");
    let pending = damage_root.join("pending");
    (root, files, damage_root, pending)
}

/// Downgrading `rusqlite` in either crate manifest below SQLite 3.51.3 would reintroduce the
/// WAL-reset corruption window that damaged the production valuation cache near checkpoint.
#[test]
fn bundled_sqlite_contains_the_wal_reset_fix() {
    assert!(
        rusqlite::version_number() >= 3_051_003,
        "bundled SQLite {} predates the WAL-reset corruption fix",
        rusqlite::version()
    );
}

/// Removing read-only validation or fresh-store creation from `open_recoverable_store` would leave
/// a corrupted rates page live, while touching broad sibling paths could mutate report rows or its
/// durable outbox during a derived-cache reset.
#[test]
fn corrupted_cache_is_quarantined_without_touching_reports() {
    // The corruption latch this test trips is process-global, and `test_state_guard` is what
    // serializes it against the tests that assert on it; without it this test can flip
    // `WRITES_BLOCKED` under an unrelated assertion.
    let _integrity = super::super::integrity::test_state_guard();
    super::super::integrity::reset_test_state();
    let (root, files, damage_root, pending) = recovery_fixture("corrupt");
    let store = open_store(&files[0]).expect("open valuation fixture");
    let transaction = store
        .unchecked_transaction()
        .expect("begin valuation seed transaction");
    for minute in 0..2_000i64 {
        transaction
            .execute(
                "INSERT INTO rates (
                     algorithm_version, quote_ordinal, minute_utc, resolved_minute_utc,
                     rate_usdt, price_basis, provider, symbol, orientation, candle_open_ms,
                     candle_close_ms, leg1_rate, fetched_at_ms
                 ) VALUES (2, 8, ?1, ?1, 1.0, 0, 'fixture', 'USDCUSDT', 0, ?2, ?3, 1.0, ?4)",
                params![
                    minute * 60,
                    minute * 60_000,
                    minute * 60_000 + 59_999,
                    minute
                ],
            )
            .expect("seed valuation rate");
    }
    transaction.commit().expect("commit valuation seed");
    crate::db::test_support::corrupt_leaf_page(store, &files[0], "rates");

    let reports_path = root.join("reports.sqlite");
    let reports = Connection::open(&reports_path).expect("open independent report fixture");
    init_report_outbox(&reports).expect("initialize report outbox");
    reports
        .execute("CREATE TABLE report_guard(value TEXT NOT NULL)", [])
        .expect("create report guard");
    reports
        .execute("INSERT INTO report_guard VALUES ('preserve-me')", [])
        .expect("seed report guard");
    stage_row(&reports, TradeSource::Typed, 9, 77).expect("seed acknowledged-independent work");

    let recovered = open_recoverable_store(files.clone(), &damage_root, &pending)
        .expect("recover corrupted valuation cache");
    assert_eq!(
        recovered
            .query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0))
            .expect("check replacement cache"),
        "ok"
    );
    assert_eq!(
        reports
            .query_row("SELECT value FROM report_guard", [], |row| {
                row.get::<_, String>(0)
            })
            .expect("read preserved report row"),
        "preserve-me"
    );
    assert_eq!(
        read_outbox(&reports, 10)
            .expect("read preserved outbox")
            .len(),
        1
    );
    let archives = std::fs::read_dir(&damage_root)
        .expect("read damage root")
        .map(|entry| entry.expect("read archive entry").path())
        .filter(|path| path != &pending)
        .collect::<Vec<_>>();
    assert_eq!(archives.len(), 1);
    assert!(archives[0].join("valuation.sqlite").is_file());
    assert!(!pending.exists());

    drop(recovered);
    drop(reports);
    std::fs::remove_dir_all(root).expect("remove corruption fixture");
}

/// Ignoring an existing pending directory in `open_recoverable_store` would abandon an already
/// retired WAL member and could create a replacement beside an incomplete old family.
#[test]
fn interrupted_family_retirement_resumes_before_replacement() {
    let (root, files, damage_root, pending) = recovery_fixture("pending");
    std::fs::create_dir_all(&pending).expect("create pending retirement");
    std::fs::write(&files[0], b"main").expect("seed main member");
    std::fs::write(&files[1], b"wal").expect("seed WAL member");
    std::fs::write(&files[2], b"shm").expect("seed SHM member");
    std::fs::rename(&files[1], pending.join("valuation.sqlite-wal"))
        .expect("simulate crash after WAL retirement");

    let archive =
        retire_store_family(&files, &damage_root, &pending).expect("resume interrupted retirement");
    assert_eq!(
        std::fs::read(archive.join("valuation.sqlite-wal")).expect("read retired WAL"),
        b"wal"
    );
    assert_eq!(
        std::fs::read(archive.join("valuation.sqlite-shm")).expect("read retired SHM"),
        b"shm"
    );
    assert_eq!(
        std::fs::read(archive.join("valuation.sqlite")).expect("read retired main"),
        b"main"
    );
    assert!(files.iter().all(|path| !path.exists()));
    assert!(!pending.exists());

    std::fs::remove_dir_all(root).expect("remove pending fixture");
}

/// Replacing `read_fail_on` with the generic report classifier would latch healthy report writes
/// when an attached valuation index is malformed; the cache-specific boundary must prove `main`
/// healthy and return a non-corruption failure instead.
#[test]
fn attached_cache_corruption_does_not_latch_report_integrity() {
    let _health = test_health_guard();
    let _integrity = super::super::integrity::test_state_guard();
    super::super::integrity::reset_test_state();
    let (root, files, _, _) = recovery_fixture("read-boundary");
    let store = open_store(&files[0]).expect("open valuation boundary fixture");
    let transaction = store
        .unchecked_transaction()
        .expect("begin trade-value seed transaction");
    for row_id in 0..2_000i64 {
        transaction
            .execute(
                "INSERT INTO trade_values (
                     source_kind, core_uid, row_id, algorithm_version, closedate,
                     quote_ordinal, profit_quote, spent_quote, rate_minute_utc,
                     rate_usdt, profit_usdt, spent_usdt, valued_at_ms
                 ) VALUES (0, 0, ?1, 1, 1700000000, 8, 1.0, 2.0, 1699999980,
                           1.0, 1.0, 2.0, 1700000100000)",
                [row_id],
            )
            .expect("seed prepared valuation");
    }
    transaction.commit().expect("commit prepared valuations");

    let reports = Connection::open_in_memory().expect("open healthy report main");
    let attach = format!(
        "ATTACH DATABASE '{}' AS valuation",
        files[0]
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "''")
    );
    reports
        .execute(&attach, [])
        .expect("attach healthy valuation");
    validate_attachment(&reports).expect("validate attachment before damage");
    crate::db::test_support::corrupt_leaf_page(store, &files[0], "sqlite_autoindex_trade_values_1");

    let mut stmt = reports
        .prepare(
            "SELECT profit_usdt FROM valuation.trade_values
             WHERE source_kind=0 AND core_uid=0 AND row_id=0",
        )
        .expect("prepare attached valuation lookup");
    let failure = match stmt.query([]) {
        Err(error) => super::super::read_fail::read_fail_on(
            &reports,
            "test: attached valuation corruption",
            error,
        ),
        Ok(mut rows) => {
            let error = rows
                .next()
                .expect_err("corrupt valuation index must fail its active cursor");
            super::super::read_fail::read_fail_on(
                &reports,
                "test: attached valuation corruption",
                error,
            )
        }
    };
    assert_eq!(failure.kind(), Some(super::super::FailKind::Other));
    assert!(!super::super::integrity::writes_blocked());
    assert!(!cache_is_healthy());

    drop(stmt);
    drop(reports);
    super::super::integrity::reset_test_state();
    std::fs::remove_dir_all(root).expect("remove read-boundary fixture");
}

/// Replacing `valuation::attach_store` corruption handling with generic `read_fail` would show the
/// report-damage banner and stop its writer when startup encounters only a malformed derived file.
#[test]
fn attach_time_cache_corruption_stays_outside_report_integrity() {
    let _health = test_health_guard();
    let _integrity = super::super::integrity::test_state_guard();
    super::super::integrity::reset_test_state();
    let (root, files, _, _) = recovery_fixture("attach-boundary");
    let store = open_store(&files[0]).expect("open attach-boundary valuation fixture");
    let transaction = store
        .unchecked_transaction()
        .expect("begin attach-boundary seed");
    for minute in 0..2_000i64 {
        transaction
            .execute(
                "INSERT INTO rates (
                     algorithm_version, quote_ordinal, minute_utc, resolved_minute_utc,
                     rate_usdt, price_basis, provider, symbol, orientation, candle_open_ms,
                     candle_close_ms, leg1_rate, fetched_at_ms
                 ) VALUES (2, 8, ?1, ?1, 1.0, 0, 'fixture', 'USDCUSDT', 0, ?2, ?3, 1.0, ?4)",
                params![
                    minute * 60,
                    minute * 60_000,
                    minute * 60_000 + 59_999,
                    minute
                ],
            )
            .expect("seed attach-boundary rate");
    }
    transaction.commit().expect("commit attach-boundary rates");
    crate::db::test_support::corrupt_leaf_page(store, &files[0], "rates");
    let reports = Connection::open_in_memory().expect("open healthy attach-boundary main");

    let attached = attach_store(&reports, &files[0]).expect("isolate derived attach corruption");

    assert!(!attached);
    assert!(!cache_is_healthy());
    assert!(!super::super::integrity::writes_blocked());
    assert_eq!(
        reports
            .query_row("PRAGMA main.quick_check(1)", [], |row| {
                row.get::<_, String>(0)
            })
            .expect("check report main after failed attach"),
        "ok"
    );

    drop(reports);
    super::super::integrity::reset_test_state();
    std::fs::remove_dir_all(root).expect("remove attach-boundary fixture");
}

/// Removing the `CACHE_LIFECYCLE.read()` guard from `valuation::attach_store` would let attachment
/// inspect a file family while recovery renames it, reopening the TOCTOU path that can misattribute
/// derived corruption to report main.
#[test]
fn attachment_waits_for_file_family_replacement() {
    let _health = test_health_guard();
    let (root, files, _, _) = recovery_fixture("attach-lifecycle");
    drop(open_store(&files[0]).expect("open lifecycle valuation fixture"));
    let reports = Connection::open_in_memory().expect("open lifecycle report fixture");
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let path = files[0].clone();
    let worker = std::thread::spawn(move || {
        ready_tx.send(()).expect("announce attachment thread");
        go_rx.recv().expect("wait for lifecycle lock");
        let result = attach_store(&reports, &path);
        done_tx.send(result).expect("publish attachment result");
    });
    ready_rx.recv().expect("wait for attachment thread");
    let replacement = CACHE_LIFECYCLE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    go_tx.send(()).expect("release attachment thread");
    assert!(matches!(
        done_rx.recv_timeout(Duration::from_millis(200)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));

    drop(replacement);
    assert!(done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("attachment resumes after replacement")
        .expect("attach healthy lifecycle fixture"));
    worker.join().expect("join attachment thread");
    std::fs::remove_dir_all(root).expect("remove lifecycle fixture");
}

/// Removing the input columns from `valuation::store_trade_value` would let a same-key report
/// upsert reuse stale USDT money; the stored row must retain every guarded source input.
#[test]
fn prepared_trade_retains_the_complete_input_fingerprint() {
    let dir = std::env::temp_dir().join(format!(
        "moonterminal-valuation-{}-{}",
        std::process::id(),
        crate::util::now_unix_ms_i64()
    ));
    std::fs::create_dir_all(&dir).expect("create valuation fixture directory");
    let path = dir.join("valuation.sqlite");
    let conn = open_store(&path).expect("open valuation fixture");
    let rate = ResolvedRate {
        quote_ordinal: 0,
        minute_utc: 1_700_000_000 / 60 * 60,
        resolved_minute_utc: 1_700_000_000 / 60 * 60,
        rate_usdt: 42_000.0,
        provider: "binance_spot".to_string(),
        symbol: "BTCUSDT".to_string(),
        orientation: RateOrientation::Direct,
        price_basis: RatePriceBasis::ExactClose,
        candle_open_ms: 1_699_999_980_000,
        candle_close_ms: 1_700_000_039_999,
        leg2_provider: None,
        leg2_symbol: None,
        leg2_orientation: None,
        leg1_rate: 42_000.0,
        leg2_rate: None,
    };
    let input = TradeInput {
        source: TradeSource::Typed,
        core_uid: 17,
        row_id: 91,
        closedate: 1_700_000_003,
        quote_ordinal: 0,
        profit_quote: 0.0125,
        spent_quote: Some(0.25),
    };
    store_trade_value(&conn, &input, &rate, 1_700_000_100_000).expect("store prepared valuation");

    let stored: (i64, i64, f64, Option<f64>, f64, Option<f64>) = conn
        .query_row(
            "SELECT closedate, quote_ordinal, profit_quote, spent_quote,
                    profit_usdt, spent_usdt
             FROM trade_values
             WHERE source_kind=0 AND core_uid=17 AND row_id=91",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .expect("read prepared valuation");
    assert_eq!(stored.0, input.closedate);
    assert_eq!(stored.1, input.quote_ordinal);
    assert_eq!(stored.2, input.profit_quote);
    assert_eq!(stored.3, input.spent_quote);
    assert_eq!(stored.4, 525.0);
    assert_eq!(stored.5, Some(10_500.0));

    drop(conn);
    std::fs::remove_dir_all(&dir).expect("remove valuation fixture directory");
}

/// Replacing either durable rate table with memory would lose successful results or retry pacing
/// after reboot and make sparse history restart its provider traffic from scratch.
#[test]
fn rate_cache_survives_reopen_without_network_state() {
    let dir = std::env::temp_dir().join(format!(
        "moonterminal-rate-cache-{}-{}",
        std::process::id(),
        crate::util::now_unix_ms_i64()
    ));
    std::fs::create_dir_all(&dir).expect("create rate-cache fixture directory");
    let path = dir.join("valuation.sqlite");
    {
        let conn = open_store(&path).expect("open rate-cache fixture");
        let rate = ResolvedRate {
            quote_ordinal: 7,
            minute_utc: 1_700_000_040,
            resolved_minute_utc: 1_700_172_840,
            rate_usdt: 0.998 * 1.001,
            provider: "hyperliquid_spot".to_string(),
            symbol: "USDH/USDC".to_string(),
            orientation: RateOrientation::Direct,
            price_basis: RatePriceBasis::SuccessorOpen,
            candle_open_ms: 1_700_172_840_000,
            candle_close_ms: 1_700_172_899_999,
            leg2_provider: Some("binance_spot".to_string()),
            leg2_symbol: Some("USDCUSDT".to_string()),
            leg2_orientation: Some(RateOrientation::Direct),
            leg1_rate: 0.998,
            leg2_rate: Some(1.001),
        };
        store_rate(&conn, &rate, 1_700_000_200_000).expect("store successful rate");
        store_rate_search(&conn, 7, 1_700_000_100, 1_700_000_160, 1_700_000_200_000)
            .expect("store retryable search");
    }
    let reopened = open_store(&path).expect("reopen rate-cache fixture");
    assert!(matches!(
        cached_rate(&reopened, 7, 1_700_000_040).expect("read successful rate"),
        Some(rate)
            if rate.resolved_minute_utc == 1_700_172_840
                && rate.price_basis == RatePriceBasis::SuccessorOpen
                && rate.leg2_symbol.as_deref() == Some("USDCUSDT")
    ));
    let covering = covering_successor_rate(&reopened, 7, 1_700_000_100)
        .expect("query covering successor")
        .expect("successor gap is reusable");
    assert_eq!(covering.minute_utc, 1_700_000_100);
    assert_eq!(covering.resolved_minute_utc, 1_700_172_840);
    assert_eq!(
        cached_rate(&reopened, 7, 1_700_000_100).expect("read unresolved rate"),
        None
    );
    assert!(
        rate_search_start(&reopened, 7, 1_700_000_100, 1_700_000_200_001)
            .expect("read persisted retry boundary")
            .is_none()
    );
    drop(reopened);
    std::fs::remove_dir_all(&dir).expect("remove rate-cache fixture directory");
}

/// `db/mod.rs:apply_message` relies on valuation outbox writes sharing the report transaction;
/// moving staging after commit would make the rollback assertion retain work and could also lose
/// a committed report change on a crash between the two independent commits.
#[test]
fn report_outbox_obeys_transaction_commit_and_rollback() {
    let mut conn = Connection::open_in_memory().expect("open report fixture");
    init_report_outbox(&conn).expect("initialize outbox");
    {
        let transaction = conn
            .transaction()
            .expect("begin rolled-back report mutation");
        stage_row(&transaction, TradeSource::Typed, 7, 11).expect("stage rolled-back row");
        transaction.rollback().expect("roll back report mutation");
    }
    assert!(read_outbox(&conn, 10)
        .expect("read empty outbox")
        .is_empty());

    {
        let transaction = conn.transaction().expect("begin committed report mutation");
        stage_row(&transaction, TradeSource::Typed, 7, 11).expect("stage committed row");
        stage_delete(&transaction, TradeSource::Legacy, 8, 12).expect("stage committed delete");
        transaction.commit().expect("commit report mutation");
    }
    let events = read_outbox(&conn, 10).expect("read committed outbox");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].action, OutboxAction::Row);
    assert_eq!(events[1].action, OutboxAction::Delete);
    assert!(events[0].seq < events[1].seq);

    ack_outbox(&conn, events[0].seq).expect("acknowledge durable prefix");
    let remaining = read_outbox(&conn, 10).expect("read remaining outbox");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].seq, events[1].seq);
}

/// Removing the `trade_values` probe from `valuation::validate_attachment` would let an existing
/// stale database without prepared valuations masquerade as usable and report zero coverage.
#[test]
fn malformed_existing_valuation_schema_fails_validation() {
    let conn = Connection::open_in_memory().expect("open report fixture");
    conn.execute_batch(
        "ATTACH DATABASE ':memory:' AS valuation;
         CREATE TABLE valuation.rates (
             algorithm_version INTEGER, quote_ordinal INTEGER, minute_utc INTEGER,
             status INTEGER, rate_usdt REAL, provider TEXT, symbol TEXT,
             orientation INTEGER, candle_open_ms INTEGER, candle_close_ms INTEGER
         );",
    )
    .expect("attach malformed valuation fixture");

    assert!(
        validate_attachment(&conn).is_err(),
        "an incomplete attached schema must fail the reader contract"
    );
}

/// Removing the primary-key checks from `validate_schema_with_prefix` would accept duplicate rate
/// keys, let valuation joins multiply report rows, and return inflated native totals.
#[test]
fn duplicate_capable_schema_is_retired_before_reader_attachment() {
    let (root, files, damage_root, pending) = recovery_fixture("missing-primary-keys");
    let malformed = Connection::open(&files[0]).expect("open malformed valuation fixture");
    malformed
        .execute_batch(
            "CREATE TABLE rates (
                 algorithm_version INTEGER NOT NULL,
                 quote_ordinal INTEGER NOT NULL,
                 minute_utc INTEGER NOT NULL,
                 resolved_minute_utc INTEGER NOT NULL,
                 rate_usdt REAL NOT NULL,
                 price_basis INTEGER NOT NULL,
                 provider TEXT NOT NULL,
                 symbol TEXT NOT NULL,
                 orientation INTEGER NOT NULL,
                 candle_open_ms INTEGER NOT NULL,
                 candle_close_ms INTEGER NOT NULL,
                 leg1_rate REAL NOT NULL,
                 leg2_provider TEXT,
                 leg2_symbol TEXT,
                 leg2_orientation INTEGER,
                 leg2_rate REAL,
                 fetched_at_ms INTEGER NOT NULL
             );
             CREATE TABLE rate_searches (
                 algorithm_version INTEGER NOT NULL,
                 quote_ordinal INTEGER NOT NULL,
                 minute_utc INTEGER NOT NULL,
                 searched_through_minute INTEGER NOT NULL,
                 next_retry_at_ms INTEGER NOT NULL,
                 attempts INTEGER NOT NULL,
                 updated_at_ms INTEGER NOT NULL,
                 PRIMARY KEY (algorithm_version, quote_ordinal, minute_utc)
             );
             CREATE TABLE trade_values (
                 source_kind INTEGER NOT NULL,
                 core_uid INTEGER NOT NULL,
                 row_id INTEGER NOT NULL,
                 algorithm_version INTEGER NOT NULL,
                 closedate INTEGER NOT NULL,
                 quote_ordinal INTEGER NOT NULL,
                 profit_quote REAL NOT NULL,
                 spent_quote REAL,
                 rate_minute_utc INTEGER NOT NULL,
                 rate_usdt REAL NOT NULL,
                 profit_usdt REAL NOT NULL,
                 spent_usdt REAL,
                 valued_at_ms INTEGER NOT NULL,
                 PRIMARY KEY (source_kind, core_uid, row_id)
             );
             INSERT INTO rates VALUES
                 (2, 8, 100, 100, 1.0, 0, 'fixture', 'USDCUSDT', 0,
                  0, 59999, 1.0, NULL, NULL, NULL, NULL, 1),
                 (2, 8, 100, 100, 1.0, 0, 'fixture', 'USDCUSDT', 0,
                  0, 59999, 1.0, NULL, NULL, NULL, NULL, 2);",
        )
        .expect("seed duplicate-capable valuation schema");
    drop(malformed);

    let recovered = open_recoverable_store(files, &damage_root, &pending)
        .expect("retire malformed schema and create replacement");
    assert_eq!(
        recovered
            .query_row("SELECT COUNT(*) FROM rates", [], |row| row.get::<_, i64>(0))
            .expect("count replacement rates"),
        0,
        "duplicate-capable storage must not survive recovery"
    );
    validate_store_schema(&recovered).expect("replacement retains exact key contracts");
    assert_eq!(
        std::fs::read_dir(&damage_root)
            .expect("read malformed-schema quarantine")
            .count(),
        1
    );

    drop(recovered);
    std::fs::remove_dir_all(root).expect("remove malformed-schema fixture");
}
