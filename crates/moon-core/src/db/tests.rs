use super::*;
use rusqlite::types::Value;
use std::cell::Cell;

/// `db/mod.rs:tune_reader` must actually raise the page-cache budget it is handed.
///
/// Dropping its `pragma_update` line leaves every report reader on SQLite's ~2 MiB default
/// against a replica of hundreds of MB: a period scan then evicts its own pages inside one
/// statement batch, so Analytics gets slower with nothing failing and nothing else to notice it.
///
/// What this does NOT cover: `open_reader` itself, which resolves its path through the global
/// `config::paths::reports_db_path()` and so cannot be called here. Deleting the
/// `tune_reader(&conn)` line from it stays green — the only guard against that is the review of
/// a four-line function.
///
/// The default is read back from an untuned connection rather than written down, so the
/// assertion cannot pass by comparing the constant with itself.
#[test]
fn a_report_reader_gets_a_larger_page_cache_than_the_default() {
    let untuned = Connection::open_in_memory().unwrap();
    let default_kib: i64 = untuned
        .pragma_query_value(None, "cache_size", |r| r.get(0))
        .unwrap();

    let tuned = Connection::open_in_memory().unwrap();
    tune_reader(&tuned);
    let tuned_kib: i64 = tuned
        .pragma_query_value(None, "cache_size", |r| r.get(0))
        .unwrap();

    assert_eq!(tuned_kib, READER_CACHE_KIB);
    // Negative form: KiB, so "bigger budget" is a more negative number.
    assert!(
        tuned_kib < default_kib,
        "tuned cache_size {tuned_kib} must exceed the default {default_kib}"
    );
}

/// `db/mod.rs:tune_reader` must leave `temp_store` and `mmap_size` at their defaults.
///
/// Setting `temp_store = MEMORY` can turn a successful sorter spill into an OOM once several
/// readers scan at once. Setting `mmap_size` can make a truncated replica surface as an OS fault
/// that `read_fail` cannot classify as corruption. Adding either setting to `tune_reader` changes
/// failure behaviour and reddens the corresponding assertion.
///
/// `mmap_size` is checked on a FILE-backed database on purpose: it configures a mapping of the
/// database file, so an in-memory connection answers the query with no row at all and an
/// assertion there would pass whatever `tune_reader` did.
#[test]
fn reader_tuning_stays_behaviour_neutral() {
    let baseline = Connection::open_in_memory().unwrap();
    let default_temp_store: i64 = baseline
        .pragma_query_value(None, "temp_store", |r| r.get(0))
        .unwrap();

    let tuned = Connection::open_in_memory().unwrap();
    tune_reader(&tuned);
    let temp_store: i64 = tuned
        .pragma_query_value(None, "temp_store", |r| r.get(0))
        .unwrap();

    assert_eq!(
        temp_store, default_temp_store,
        "temp_store must stay at the default; MEMORY trades a disk spill for an OOM"
    );

    let path = test_support::temp_db("reader-tuning");
    {
        let on_disk = Connection::open(&path).unwrap();
        let default_mmap: i64 = on_disk
            .pragma_query_value(None, "mmap_size", |r| r.get(0))
            .unwrap();
        tune_reader(&on_disk);
        let mmap: i64 = on_disk
            .pragma_query_value(None, "mmap_size", |r| r.get(0))
            .unwrap();
        assert_eq!(
            mmap, default_mmap,
            "mmap_size must stay at the default; a mapping hides corruption from read_fail"
        );
    }
    test_support::remove_db(&path);
}

/// `db/mod.rs:publish_report_commit` must set the dirty bit after the generation bump; removing
/// that store leaves an open Report or Analytics view stale until an unrelated Backend repaint.
#[test]
fn committed_batch_publication_is_bounded_and_advances_generation() {
    let generation = AtomicU64::new(0);
    let dirty = AtomicBool::new(false);

    publish_report_commit(&generation, &dirty);
    publish_report_commit(&generation, &dirty);

    assert_eq!(generation.load(Ordering::Relaxed), 2);
    assert!(dirty.swap(false, Ordering::Acquire));
    assert!(!dirty.swap(false, Ordering::Acquire));
}

/// `db/mod.rs:publish_after_generation` must advance the generation before invoking
/// the edge publisher; swapping those statements lets the UI consume an edge while
/// still observing the previous report revision.
#[test]
fn committed_batch_publishes_only_after_generation_advance() {
    let generation = AtomicU64::new(41);
    let observed = Cell::new(0);

    publish_after_generation(&generation, || {
        observed.set(generation.load(Ordering::Relaxed));
    });

    assert_eq!(observed.get(), 42);
}

/// `db/mod.rs:commit_stateful_batch` must retry the same owned work with a fresh
/// candidate state; returning after the first failed commit loses the batch, while
/// reusing its candidate applies in-memory effects twice.
#[test]
fn failed_commit_retries_without_leaking_candidate_state() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE sample(value INTEGER)", [])
        .unwrap();
    let attempts = Cell::new(0usize);
    let mut state = 0usize;

    let committed_attempt = commit_stateful_batch(&conn, &mut state, |transaction, candidate| {
        let attempt = attempts.get() + 1;
        attempts.set(attempt);
        *candidate += 1;
        transaction
            .execute("INSERT INTO sample VALUES (1)", [])
            .unwrap();
        if attempt == 1 {
            transaction.execute_batch("ROLLBACK").unwrap();
        }
        Ok(attempt)
    })
    .unwrap();

    assert_eq!(committed_attempt, 2);
    assert_eq!(state, 1);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM sample", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

/// `db/mod.rs:commit_stateful_batch_until_success` must begin a new retry round after the
/// transaction-level allowance is exhausted; returning the first error closes the sole writer
/// and makes every later live report event disappear until process restart.
#[test]
fn exhausted_retry_round_keeps_the_owned_batch() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE sample(value INTEGER)", [])
        .unwrap();
    let attempts = Cell::new(0usize);
    let waits = Cell::new(0usize);
    let mut state = 0usize;

    let committed_attempt = commit_stateful_batch_until_success(
        &conn,
        &mut state,
        |transaction, candidate| {
            let attempt = attempts.get() + 1;
            attempts.set(attempt);
            *candidate += 1;
            transaction.execute("INSERT INTO sample VALUES (1)", [])?;
            if attempt <= REPORT_BATCH_ATTEMPTS {
                transaction.execute_batch("ROLLBACK")?;
            }
            Ok(attempt)
        },
        |_| false,
        |_, _| waits.set(waits.get() + 1),
    )
    .unwrap();

    assert_eq!(committed_attempt, REPORT_BATCH_ATTEMPTS + 1);
    assert_eq!(waits.get(), 1);
    assert_eq!(state, 1);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM sample", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

/// `db/mod.rs:commit_stateful_batch_until_success` must abort a corruption-class owned batch.
///
/// Replacing the production abort predicate with `false` invokes the wait callback below after the
/// transaction retry allowance and loops forever in production, so the named zero-wait assertion
/// protects the user-visible guarantee that a damaged replica stops receiving writes and ACKs.
#[test]
fn corruption_aborts_the_owned_batch_without_entering_backoff() {
    let _state_guard = integrity::test_state_guard();
    integrity::reset_test_state();
    let conn = Connection::open_in_memory().unwrap();
    let attempts = Cell::new(0usize);
    let waits = Cell::new(0usize);
    let mut state = 0usize;

    let result = commit_stateful_batch_until_success(
        &conn,
        &mut state,
        |_transaction, candidate| {
            attempts.set(attempts.get() + 1);
            *candidate += 1;
            Err::<(), _>(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ErrorCode::DatabaseCorrupt,
                    extended_code: 11,
                },
                None,
            ))
        },
        integrity::writer_should_stop,
        |_, _| waits.set(waits.get() + 1),
    );

    assert!(matches!(
        result,
        Err(rusqlite::Error::SqliteFailure(error, _))
            if error.code == rusqlite::ErrorCode::DatabaseCorrupt
    ));
    assert_eq!(attempts.get(), REPORT_BATCH_ATTEMPTS);
    assert_eq!(waits.get(), 0);
    assert_eq!(state, 0);
    integrity::reset_test_state();
}

/// `db/mod.rs:commit_stateful_batch_until_success` must bound non-corruption retry rounds.
///
/// Raising `REPORT_BATCH_MAX_FAILED_ROUNDS` by one makes the independent attempt-count assertion
/// fail; without any ceiling, one permanent I/O or permission error wedges the bounded producer
/// channel forever while no report page can be acknowledged.
#[test]
fn permanent_non_corruption_failure_closes_the_writer_after_the_bound() {
    let conn = Connection::open_in_memory().unwrap();
    let attempts = Cell::new(0usize);
    let waits = Cell::new(0usize);
    let mut state = 0usize;

    let result = commit_stateful_batch_until_success(
        &conn,
        &mut state,
        |_transaction, candidate| {
            attempts.set(attempts.get() + 1);
            *candidate += 1;
            Err::<(), _>(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ErrorCode::ReadOnly,
                    extended_code: 8,
                },
                None,
            ))
        },
        |_| false,
        |_, _| waits.set(waits.get() + 1),
    );

    assert!(result.is_err());
    assert_eq!(
        attempts.get(),
        REPORT_BATCH_ATTEMPTS * REPORT_BATCH_MAX_FAILED_ROUNDS as usize
    );
    assert_eq!(waits.get(), REPORT_BATCH_MAX_FAILED_ROUNDS as usize - 1);
    assert_eq!(state, 0);
}

/// `db/mod.rs:passive_wal_checkpoint` must return SQLite's progress tuple; replacing
/// it with an ignored row hides the pinned reader that prevents full checkpointing.
#[test]
fn passive_checkpoint_reports_a_pinned_reader() {
    static NEXT_DB: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "moonterminal-checkpoint-{}-{}.sqlite",
        std::process::id(),
        NEXT_DB.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&path);
    let writer = Connection::open(&path).unwrap();
    writer.pragma_update(None, "journal_mode", "WAL").unwrap();
    writer
        .execute_batch("CREATE TABLE sample(value INTEGER); INSERT INTO sample VALUES (0);")
        .unwrap();
    let reader = Connection::open(&path).unwrap();
    let snapshot = reader.unchecked_transaction().unwrap();
    let _: i64 = snapshot
        .query_row("SELECT value FROM sample", [], |row| row.get(0))
        .unwrap();
    for value in 1..=32 {
        writer
            .execute("UPDATE sample SET value=?1", [value])
            .unwrap();
    }

    let status = passive_wal_checkpoint(&writer).unwrap();

    assert_eq!(status.busy, 0);
    assert!(status.log_frames > status.checkpointed_frames);
    drop(snapshot);
    drop(reader);
    drop(writer);
    let _ = std::fs::remove_file(path);
}

/// `db/mod.rs:with_read_snapshot` must keep the transaction around the whole callback; replacing
/// it with `read(conn)` lets the second statement observe a WAL commit that the first did not,
/// producing internally inconsistent compound Analytics results.
#[test]
fn compound_read_keeps_one_wal_snapshot() {
    static NEXT_DB: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "moonterminal-read-snapshot-{}-{}.sqlite",
        std::process::id(),
        NEXT_DB.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&path);
    let writer = Connection::open(&path).unwrap();
    writer.pragma_update(None, "journal_mode", "WAL").unwrap();
    writer
        .execute_batch("CREATE TABLE sample(value INTEGER); INSERT INTO sample VALUES (1);")
        .unwrap();
    let reader = Connection::open(&path).unwrap();

    let (before, after) = with_read_snapshot(&reader, |snapshot| {
        let before: i64 = snapshot
            .query_row("SELECT value FROM sample", [], |row| row.get(0))
            .map_err(|error| read_fail("test: first snapshot read", error))?;
        writer.execute("UPDATE sample SET value = 2", []).unwrap();
        let after: i64 = snapshot
            .query_row("SELECT value FROM sample", [], |row| row.get(0))
            .map_err(|error| read_fail("test: second snapshot read", error))?;
        Ok((before, after))
    })
    .unwrap();

    assert_eq!((before, after), (1, 1));
    drop(reader);
    drop(writer);
    let _ = std::fs::remove_file(path);
}

/// Replica indexes are also created for databases whose columns predate the index code.
///
/// Creating them only after a successful ALTER lost the index permanently because
/// the schema loop skips columns that already exist.
#[test]
fn rep_indexes_created_for_preexisting_columns() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();
    // Replica skeleton with columns but NO indexes, matching an older database.
    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER NOT NULL,
            core_name TEXT NOT NULL, newrecid INTEGER NOT NULL,
            PRIMARY KEY (core_uid, newrecid));
         ALTER TABLE orders_rep ADD COLUMN closedate INTEGER;
         ALTER TABLE orders_rep ADD COLUMN strategyid INTEGER;
         ALTER TABLE orders_rep ADD COLUMN buydate INTEGER;",
    )
    .unwrap();
    test_support::rep_init(&conn);
    // Against the declaration itself, so a new entry there cannot be missed here.
    for (name, _) in rep::REP_INDEXES {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                [name],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "индекс {name} не создан после init");
    }
    // The planner actually uses the index for the default period filter.
    let plan: String = conn
        .query_row(
            "EXPLAIN QUERY PLAN SELECT * FROM orders_rep
             WHERE closedate >= 1 AND closedate < 2 AND closedate > 0",
            [],
            |r| r.get(3),
        )
        .unwrap();
    assert!(
        plan.contains("idx_rep_closedate"),
        "план без индекса: {plan}"
    );
}

/// A period query narrowed to SOME cores must resolve the period INSIDE the index.
///
/// This pins the plan that made Analytics take ~10 s on a 458 MB replica: without
/// `idx_rep_core_close` the planner serves the `core_uid` equality from a core-leading index
/// and then tests `closedate` against the table for every row those cores ever produced — so
/// "today" costs as much as "all history". The source comes from `unified_from`, the production
/// builder, rather than a lookalike assembled here: a filter change that moved `closedate` out
/// of an indexable position would otherwise leave this green while the panel went back to ten
/// seconds.
#[test]
fn rep_core_and_period_filter_uses_composite_index() {
    let path = test_support::temp_db("core-close");
    let conn = test_support::build_replica(&path, &test_support::spread_rows(1_700_000_000, 30));
    let q = analytics::Query {
        from: 1,
        to: 2,
        cores: vec![1, 3, 7],
        ..Default::default()
    };
    let src = analytics::unified_from(&conn, &q)
        .ok()
        .flatten()
        .expect("реплика — годный источник");
    let mut stmt = conn
        .prepare(&format!("EXPLAIN QUERY PLAN SELECT o.closedate FROM {src}"))
        .unwrap();
    // The source is a UNION branch inside a subquery, so its plan is several rows.
    let plan = stmt
        .query_map(rusqlite::params![1_i64, 2_i64], |r| r.get::<_, String>(3))
        .unwrap()
        .map(|r| r.unwrap())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        plan.contains("idx_rep_core_close"),
        "период с фильтром ядер идёт мимо составного индекса: {plan}"
    );
    drop(stmt);
    drop(conn);
    test_support::remove_db(&path);
}

/// Cores come out newest-first, each labelled by its CURRENT name.
///
/// Both halves are caller-visible, though not equally: the UI re-ranks this list by config
/// order (`CoreOrder::from_db`), so the recency order decides only among cores the config does
/// not know — which is exactly where it is the last word. The NAME has no such fallback: a core
/// renamed on the server must not keep showing the name it carried at its first trade.
///
/// The loose index scan states both outright, where the previous `GROUP BY` leaned on SQLite's
/// bare-column rule for a LONE min/max aggregate — a second one voids it silently, with no
/// error anywhere. Note what this does NOT pin: the old statement satisfies it too. It guards
/// the contract the rewrite had to preserve, not the plan — that one is measured, not asserted
/// (250 ms → under 1 ms on a 458 MB replica).
#[test]
fn distinct_cores_lists_newest_first_with_the_current_name() {
    let path = test_support::temp_db("cores");
    let conn = test_support::build_replica(&path, &[]);
    conn.execute_batch(
        "INSERT INTO orders_rep (core_uid, core_name, newrecid) VALUES
            (7, 'ИМЯ ДО ПЕРЕИМЕНОВАНИЯ', 1), (7, 'BinF1', 2),
            (3, 'Gate', 5),
            (9, 'HLSpot', 3);",
    )
    .unwrap();
    let got = distinct_cores(&conn).expect("список ядер читается");
    assert_eq!(
        got,
        vec![
            (3, "Gate".to_string()),
            (9, "HLSpot".to_string()),
            (7, "BinF1".to_string()),
        ],
        "ядра — от последнего активного к первому, имя каждого — из его самой свежей строки"
    );
    drop(conn);
    test_support::remove_db(&path);
}

/// Both report sources feed the selector, and a core present in both is listed once.
///
/// The two branches are DIFFERENT statements — the legacy table's recency key is indexed
/// nowhere, so it keeps the grouped pass while the replica gets the loose index scan — and this
/// is the only test that runs the legacy one. An untested branch is how the transitional source
/// quietly stops showing up in the core selector of a half-migrated install.
///
/// Core 5 carries TWO legacy rows on purpose, and its NEWEST row is the one with the LOWER
/// `db_id`: that is what exercises the bare-column rule the grouped branch leans on, and it
/// separates "the name from the newest row" from "the name from the last row in key order",
/// which a fixture with both ascending together cannot tell apart. Core 8 exists so the legacy
/// branch contributes two cores of its own — with only one, its ORDER BY could be reversed and
/// this test would not notice.
#[test]
fn distinct_cores_merges_legacy_source_without_duplicating_a_core() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();
    conn.execute_batch(
        "CREATE TABLE closed_sell_reports (
            core_uid INTEGER NOT NULL, core_name TEXT NOT NULL, db_id INTEGER NOT NULL,
            coin TEXT, profitbtc REAL, closedate INTEGER,
            created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL,
            PRIMARY KEY (core_uid, db_id));
         INSERT INTO closed_sell_reports
            (core_uid, core_name, db_id, coin, profitbtc, closedate, created_ms, updated_ms)
         VALUES (1, 'BB1', 42, 'BTCUSDT', 0.5, 1780000000, 0, 10),
                (5, 'Gate', 43, 'BTCUSDT', 0.5, 1780000000, 0, 30),
                (5, 'Gate до переименования', 44, 'BTCUSDT', 0.5, 1780000000, 0, 20),
                (8, 'QQ', 45, 'BTCUSDT', 0.5, 1780000000, 0, 15);",
    )
    .unwrap();
    test_support::rep_init(&conn);
    // Core 1 is ALSO on typed replication, under a name the server has since changed.
    conn.execute(
        "INSERT INTO orders_rep (core_uid, core_name, newrecid) VALUES (1, 'BB1 переименовано', 3)",
        [],
    )
    .unwrap();
    let got = distinct_cores(&conn).expect("список ядер читается");
    assert_eq!(
        got,
        vec![
            (1, "BB1 переименовано".to_string()),
            (5, "Gate".to_string()),
            (8, "QQ".to_string()),
        ],
        "реплика идёт первой и вытесняет легаси-строку того же ядра; легаси добавляет свои \
         в порядке своей свежести"
    );
}

/// The transitional reader exposes rows from the LEGACY table and typed replica together.
///
/// Legacy `db_id` appears as `id`. After `SyncComplete` for the final legacy core,
/// the table is dropped and the reader uses only the replica.
#[test]
fn union_reader_and_legacy_drop() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();

    // Legacy table plus one row for core 1. `init_db` NO LONGER creates this table;
    // users already have it during migration. Recreate the minimal schema BEFORE
    // `rep::init` so it sees `legacy_exists=true` and can drop it on SyncComplete.
    conn.execute_batch(
        "CREATE TABLE closed_sell_reports (
            core_uid INTEGER NOT NULL, core_name TEXT NOT NULL, db_id INTEGER NOT NULL,
            coin TEXT, profitbtc REAL, closedate INTEGER,
            created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL,
            PRIMARY KEY (core_uid, db_id));
         INSERT INTO closed_sell_reports
            (core_uid, core_name, db_id, coin, profitbtc, closedate, created_ms, updated_ms)
         VALUES (1, 'BB1', 42, 'BTCUSDT', 0.5, 1780000000, 0, 0);",
    )
    .unwrap();

    let mut st = test_support::rep_init(&conn);

    // Replica row for core 2, with columns as the core schema would extend them.
    for ddl in [
        "ALTER TABLE orders_rep ADD COLUMN coin TEXT",
        "ALTER TABLE orders_rep ADD COLUMN profitbtc REAL",
        "ALTER TABLE orders_rep ADD COLUMN closedate INTEGER",
        "ALTER TABLE orders_rep ADD COLUMN id INTEGER",
    ] {
        conn.execute(ddl, []).unwrap();
    }
    conn.execute(
        "INSERT INTO orders_rep (core_uid, core_name, newrecid, coin, profitbtc, closedate, id)
         VALUES (2, 'Rep', 7, 'ETHUSDT', 1.5, 1780000100, 99)",
        [],
    )
    .unwrap();

    let cols = display_columns(&conn).expect("схема читается");
    assert!(cols.iter().any(|c| c == "id"), "cols: {cols:?}");
    assert!(!cols.iter().any(|c| c == "db_id" || c == "newrecid"));

    let t = query_reports(&conn, &ReportFilter::default(), "closedate", true, 10)
        .expect("выборка читается");
    assert_eq!(t.rows.len(), 2);
    assert!(t.core_uids.contains(&1) && t.core_uids.contains(&2));
    // Legacy `db_id` is exposed in the `id` column.
    let id_ix = t.cols.iter().position(|c| c == "id").unwrap();
    let legacy_row = t.core_uids.iter().position(|u| *u == 1).unwrap();
    assert_eq!(t.rows[legacy_row][id_ix], Value::Integer(42));

    let (profit, count) = query_totals(&conn, &ReportFilter::default()).expect("итоги читаются");
    assert_eq!(count, 2);
    assert!((profit - 2.0).abs() < 1e-9);

    // SyncComplete for core 1 purges its legacy rows; the empty table is then dropped.
    rep::apply_sync_complete(&conn, &mut st, 1).unwrap();
    let legacy_left = table_columns_res(&conn).expect("схема читается");
    assert!(legacy_left.is_empty(), "легаси-таблица должна быть снесена");
    let t = query_reports(&conn, &ReportFilter::default(), "closedate", true, 10)
        .expect("выборка читается после сноса легаси");
    assert_eq!(t.rows.len(), 1);
    assert_eq!(t.core_uids, vec![2]);
    // The `legacy_dropped` marker prevents `init_db` from resurrecting the legacy table.
    init_db(&conn).unwrap();
    assert!(table_columns_res(&conn).expect("схема читается").is_empty());
}

/// Index-page damage fails the report query instead of returning a table
/// that the panel or file export could mistake for a complete result.
#[test]
fn corrupt_replica_fails_report_query_instead_of_truncating() {
    let path = test_support::temp_db("report-rows");
    let day = 1_780_000_000i64 / 86_400 * 86_400;
    let conn = test_support::build_replica(&path, &test_support::spread_rows(day, 2000));

    let filter = ReportFilter {
        date_from: Some(day),
        date_to: Some(day + 2000 * 60),
        emulator: Some(false),
        ..Default::default()
    };
    // The fixture is healthy first, so the assertions below cannot pass for
    // a trivial reason (an empty or unreadable database).
    let before =
        query_reports(&conn, &filter, "closedate", true, 5_000).expect("до порчи выборка читается");
    assert_eq!(before.rows.len(), 2000);

    // Pin the plan: if the planner stops using this index, the test must fail
    // loudly rather than quietly stop exercising the damaged page.
    let plan: String = conn
        .query_row(
            "EXPLAIN QUERY PLAN SELECT core_uid FROM orders_rep
             WHERE closedate IS NOT NULL AND closedate >= 1 AND closedate <= 2
             ORDER BY closedate DESC",
            [],
            |r| r.get(3),
        )
        .unwrap();
    assert!(
        plan.contains("idx_rep_closedate"),
        "план без индекса: {plan}"
    );

    test_support::corrupt_leaf_page(conn, &path, "idx_rep_closedate");
    let conn = Connection::open(&path).expect("битая БД всё ещё открывается");

    let res = query_reports(&conn, &filter, "closedate", true, 5_000);
    assert!(
        !matches!(res, Ok(_)),
        "сбой чтения обязан вернуть ошибку, а не частичную/пустую таблицу: \
         такой файл экспорта неотличим от полного"
    );
    assert!(
        matches!(
            res,
            Err(ReadFail::Failed {
                kind: FailKind::Corrupt,
                ..
            })
        ),
        "порча должна классифицироваться как Corrupt"
    );

    drop(conn);
    test_support::remove_db(&path);
}

/// Aggregate corruption returns an error rather than the zero totals of a
/// healthy empty period.
#[test]
fn corrupt_replica_fails_report_totals_instead_of_zeroing() {
    let path = test_support::temp_db("report-totals");
    let day = 1_780_000_000i64 / 86_400 * 86_400;
    let conn = test_support::build_replica(&path, &test_support::spread_rows(day, 2000));

    // No date bounds: the aggregate must read the table itself, so damaging a
    // TABLE leaf (not an index) is what this scan is guaranteed to hit.
    let filter = ReportFilter {
        emulator: Some(false),
        ..Default::default()
    };
    let (profit, count) = query_totals(&conn, &filter).expect("до порчи итоги читаются");
    assert_eq!(count, 2000);
    assert!(profit.is_finite());

    test_support::corrupt_leaf_page(conn, &path, "orders_rep");
    let conn = Connection::open(&path).expect("битая БД всё ещё открывается");

    let res = query_totals(&conn, &filter);
    assert!(
        !matches!(res, Ok(_)),
        "сбой агрегата обязан вернуть ошибку, а не (0.0, 0)"
    );

    drop(conn);
    test_support::remove_db(&path);
}

/// Soft-deleted rows leave the table and the totals by default; the deleted-only
/// mode inverts the selection to exactly those rows.
///
/// The legacy table has no `deleted` column: its rows pass unfiltered by default
/// and contribute nothing in deleted-only mode. NULL counts as not deleted.
#[test]
fn deleted_filter_default_hides_and_only_mode_inverts() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();

    // Legacy source without a `deleted` column, one row.
    conn.execute_batch(
        "CREATE TABLE closed_sell_reports (
            core_uid INTEGER NOT NULL, core_name TEXT NOT NULL, db_id INTEGER NOT NULL,
            coin TEXT, profitbtc REAL, closedate INTEGER,
            created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL,
            PRIMARY KEY (core_uid, db_id));
         INSERT INTO closed_sell_reports
            (core_uid, core_name, db_id, coin, profitbtc, closedate, created_ms, updated_ms)
         VALUES (1, 'Legacy', 1, 'LEGACY', 1.0, 1780000000, 0, 0);",
    )
    .unwrap();

    test_support::rep_init(&conn);

    // Replica rows: kept (0), soft-deleted (1), and NULL, which counts as kept.
    conn.execute_batch(
        "ALTER TABLE orders_rep ADD COLUMN coin TEXT;
         ALTER TABLE orders_rep ADD COLUMN profitbtc REAL;
         ALTER TABLE orders_rep ADD COLUMN closedate INTEGER;
         ALTER TABLE orders_rep ADD COLUMN deleted INTEGER;
         INSERT INTO orders_rep (core_uid, core_name, newrecid, coin, profitbtc, closedate, deleted)
         VALUES (2, 'Rep', 1, 'KEPT',   10.0, 1780000100, 0),
                (2, 'Rep', 2, 'GONE',  -50.0, 1780000200, 1),
                (2, 'Rep', 3, 'NULLD',  2.0, 1780000300, NULL);",
    )
    .unwrap();

    let coins = |f: &ReportFilter| -> Vec<String> {
        let t = query_reports(&conn, f, "closedate", true, 10).expect("выборка читается");
        let ix = t.cols.iter().position(|c| c == "coin").unwrap();
        let mut out: Vec<String> = t
            .rows
            .iter()
            .map(|r| match &r[ix] {
                Value::Text(s) => s.clone(),
                v => panic!("coin не текст: {v:?}"),
            })
            .collect();
        out.sort();
        out
    };

    // Default: the soft-deleted row is gone from rows and totals alike.
    let visible = ReportFilter::default();
    assert_eq!(coins(&visible), ["KEPT", "LEGACY", "NULLD"]);
    let (profit, count) = query_totals(&conn, &visible).expect("итоги читаются");
    assert_eq!(count, 3);
    assert!((profit - 13.0).abs() < 1e-9, "profit={profit}");

    // Deleted-only: exactly the soft-deleted row; the legacy source yields nothing.
    let only = ReportFilter {
        deleted_only: true,
        ..Default::default()
    };
    assert_eq!(coins(&only), ["GONE"]);
    let (profit, count) = query_totals(&conn, &only).expect("итоги читаются");
    assert_eq!(count, 1);
    assert!((profit + 50.0).abs() < 1e-9, "profit={profit}");
}

/// Protects `max_core_uid`: it must fold BOTH report schemas, and a source whose table does not
/// exist yet must read as "nothing here" rather than failing the probe.
///
/// The plausible edit: dropping the `core_uid` column guard and querying every source
/// `read_sources_res` reports. That function always reports the modern table, which does not
/// exist until `rep::init` has run, so a replica that has only ever held legacy rows would turn
/// a healthy "no rows" into a hard read failure.
///
/// Consequence: the caller treats a failure as "this store contributes nothing", which is
/// precisely the state in which a deleted core's uid gets handed to a new core along with its
/// trades and P&L.
#[test]
fn max_core_uid_folds_both_report_schemas() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();

    // Neither the legacy table (init_db no longer creates it) nor `orders_rep` exists yet;
    // an empty store is not a failure.
    assert!(
        matches!(max_core_uid(&conn), Ok(None)),
        "an empty replica must read as no rows, never as a read failure"
    );

    // Legacy rows still exist on the transition period; seed the table here (init_db no
    // longer creates it) to exercise the cross-schema uid fold.
    conn.execute_batch(
        "CREATE TABLE closed_sell_reports (
            core_uid INTEGER NOT NULL, core_name TEXT NOT NULL, db_id INTEGER NOT NULL,
            created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL,
            PRIMARY KEY (core_uid, db_id));
         INSERT INTO closed_sell_reports (core_uid, core_name, db_id, created_ms, updated_ms)
         VALUES (12, 'legacy', 1, 0, 0);",
    )
    .unwrap();
    assert!(
        matches!(max_core_uid(&conn), Ok(Some(12))),
        "a uid living only in the legacy schema still has to raise the mark"
    );

    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER NOT NULL,
            core_name TEXT NOT NULL, newrecid INTEGER NOT NULL,
            PRIMARY KEY (core_uid, newrecid));
         INSERT INTO orders_rep VALUES (7, 'modern', 1);",
    )
    .unwrap();
    assert!(
        matches!(max_core_uid(&conn), Ok(Some(12))),
        "the mark is the maximum ACROSS schemas, not the last one queried"
    );

    conn.execute("INSERT INTO orders_rep VALUES (30, 'modern', 2)", [])
        .unwrap();
    assert!(matches!(max_core_uid(&conn), Ok(Some(30))));
}

/// A core-broadcast bulk soft-delete flips `deleted` for exactly the rows named by its ranges
/// and singles, scoped to the core, and a later restore (`deleted = false`) clears it back —
/// so the Report/Analytics filters see the same set the core did, and an undo is possible.
#[test]
fn set_deleted_flips_named_rows_and_scopes_to_core() {
    use moonproto::{ReportRecIdRange, ReportRowsDeleted};

    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();
    // Create the replica WITH `deleted` before rep_init, so the schema cache (`st.cols`) carries
    // it — the state apply_schema leaves once the core has declared the column, before any
    // RowsDeleted arrives.
    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER NOT NULL, core_name TEXT NOT NULL,
            newrecid INTEGER NOT NULL, deleted INTEGER, PRIMARY KEY (core_uid, newrecid));",
    )
    .unwrap();
    let st = test_support::rep_init(&conn);

    // newrecid 1..=6 for the target core 2, plus a decoy row on core 3 that shares a newrecid
    // inside the affected range — it must stay untouched (the op is per core).
    conn.execute_batch(
        "INSERT INTO orders_rep (core_uid, core_name, newrecid, deleted) VALUES
            (2, 'Rep', 1, 0), (2, 'Rep', 2, 0), (2, 'Rep', 3, 0),
            (2, 'Rep', 4, 0), (2, 'Rep', 5, 0), (2, 'Rep', 6, 0),
            (3, 'Other', 3, 0);",
    )
    .unwrap();

    let deleted_of = |core: i64, rec: i64| -> i64 {
        conn.query_row(
            "SELECT deleted FROM orders_rep WHERE core_uid=?1 AND newrecid=?2",
            rusqlite::params![core, rec],
            |r| r.get(0),
        )
        .unwrap()
    };

    // Soft-delete the range 2..=4 and the single 6; leave 1 and 5 alone.
    let del = ReportRowsDeleted::new(true, [ReportRecIdRange::new(2, 4)], [6]);
    super::rep::apply_set_deleted(&conn, &st, 2, &del).unwrap();

    for rec in [2, 3, 4, 6] {
        assert_eq!(
            deleted_of(2, rec),
            1,
            "rec {rec} должен быть помечен удалённым"
        );
    }
    for rec in [1, 5] {
        assert_eq!(deleted_of(2, rec), 0, "rec {rec} вне набора — не трогать");
    }
    assert_eq!(
        deleted_of(3, 3),
        0,
        "строка другого ядра с тем же newrecid не затрагивается"
    );

    // Restore the same set: the flag clears, proving this is a reversible flag, not a DELETE.
    let restore = ReportRowsDeleted::new(false, [ReportRecIdRange::new(2, 4)], [6]);
    super::rep::apply_set_deleted(&conn, &st, 2, &restore).unwrap();
    for rec in 1..=6 {
        assert_eq!(deleted_of(2, rec), 0, "rec {rec} восстановлен");
    }

    // Every original row still exists — a restore must not have dropped anything.
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM orders_rep WHERE core_uid=2",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 6, "soft-delete/restore не удаляет строки");
}

/// A core whose replica schema never declared `deleted` gets a clean no-op — not a
/// "no such column" error per event — mirroring the read side's `has("deleted")` guard.
#[test]
fn set_deleted_without_column_is_a_noop() {
    use moonproto::{ReportRecIdRange, ReportRowsDeleted};

    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();
    // Skeleton only: rep_init creates (core_uid, core_name, newrecid) and NO `deleted` column,
    // so the schema cache does not carry it.
    let st = test_support::rep_init(&conn);
    conn.execute(
        "INSERT INTO orders_rep (core_uid, core_name, newrecid) VALUES (2, 'Rep', 3)",
        [],
    )
    .unwrap();

    let del = ReportRowsDeleted::new(true, [ReportRecIdRange::new(1, 5)], [3]);
    // The absent column must not surface as an error, and must not touch the row.
    super::rep::apply_set_deleted(&conn, &st, 2, &del)
        .expect("отсутствие колонки — no-op, не ошибка");
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM orders_rep", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "no-op не трогает строки");
}

/// `query_reports` returns `rec_ids` parallel to `rows`: the replica `newrecid` for replica rows
/// and `0` for a legacy row that has none. The deletion-mode checkboxes address rows by this id,
/// so a wrong or misaligned value would soft-delete the wrong trade.
#[test]
fn query_reports_exposes_rec_ids_aligned_to_rows() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();

    // Legacy source: one row, no `newrecid` -> expected rec_id 0.
    conn.execute_batch(
        "CREATE TABLE closed_sell_reports (
            core_uid INTEGER NOT NULL, core_name TEXT NOT NULL, db_id INTEGER NOT NULL,
            coin TEXT, closedate INTEGER, created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL,
            PRIMARY KEY (core_uid, db_id));
         INSERT INTO closed_sell_reports (core_uid, core_name, db_id, coin, closedate, created_ms, updated_ms)
         VALUES (1, 'Legacy', 42, 'ZZZ', 1780000000, 0, 0);",
    )
    .unwrap();

    test_support::rep_init(&conn);

    // Replica rows carry a real `newrecid`; the coin distinguishes each row so the oracle can map
    // it back to the newrecid it was inserted with, independently of the projection under test.
    conn.execute_batch(
        "ALTER TABLE orders_rep ADD COLUMN coin TEXT;
         ALTER TABLE orders_rep ADD COLUMN closedate INTEGER;
         INSERT INTO orders_rep (core_uid, core_name, newrecid, coin, closedate) VALUES
            (2, 'Rep', 7, 'AAA', 1780000100),
            (2, 'Rep', 8, 'BBB', 1780000200);",
    )
    .unwrap();

    let t = query_reports(&conn, &ReportFilter::default(), "coin", false, 10).expect("выборка");
    assert_eq!(t.rows.len(), 3, "две строки реплики + одна легаси");
    assert_eq!(t.rec_ids.len(), t.rows.len(), "rec_ids параллельны rows");
    assert_eq!(
        t.core_uids.len(),
        t.rows.len(),
        "core_uids параллельны rows"
    );

    // Oracle: expected newrecid per coin, from the inserts above (legacy has none -> 0).
    let coin_ix = t
        .cols
        .iter()
        .position(|c| c == "coin")
        .expect("есть колонка coin");
    for i in 0..t.rows.len() {
        let coin = match &t.rows[i][coin_ix] {
            Value::Text(s) => s.as_str(),
            v => panic!("coin не текст: {v:?}"),
        };
        let expected = match coin {
            "AAA" => 7,
            "BBB" => 8,
            "ZZZ" => 0, // legacy: not soft-deletable
            other => panic!("неожиданная монета {other}"),
        };
        assert_eq!(
            t.rec_ids[i], expected,
            "rec_id строки {coin} должен быть {expected}"
        );
    }
}
