use super::*;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;

/// Losing watchdog classification exposes SQLite's raw interruption as the banner detail.
#[test]
fn watchdog_interruption_is_a_typed_timeout() {
    let interrupted = || rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(9), None);
    assert_eq!(
        classify_scan_error(interrupted(), true),
        Integrity::TimeLimitExceeded
    );
    assert!(matches!(
        classify_scan_error(interrupted(), false),
        Integrity::CheckFailed(_)
    ));
}

/// A deadline must never hide confirmed corruption or relabel unrelated failures as a timeout.
#[test]
fn watchdog_does_not_reclassify_damage_or_other_errors() {
    let corrupt = rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(11), None);
    assert!(matches!(
        classify_scan_error(corrupt, true),
        Integrity::Damaged(_)
    ));
    let unreadable = rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(14), None);
    assert!(matches!(
        classify_scan_error(unreadable, true),
        Integrity::CheckFailed(_)
    ));
}

/// Build a process-scoped temporary directory for one test scenario.
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moon-integrity-{}-{}", tag, std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// A missing replica produces the silent `NotPresent` verdict.
#[test]
fn absent_file_is_not_present() {
    let p = temp_dir("absent").join("nope.sqlite");
    let _ = std::fs::remove_file(&p);
    assert_eq!(run(&p), Integrity::NotPresent);
}

/// A healthy replica passes and index-page damage produces diagnostics.
#[test]
fn healthy_db_is_ok_and_damaged_db_is_reported() {
    let dir = temp_dir("healthy");
    let path = dir.join("reports.sqlite");
    for suf in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", path.display(), suf));
    }

    let conn = Connection::open(&path).unwrap();
    super::super::init_db(&conn).unwrap();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS orders_rep (core_uid INTEGER NOT NULL,
            core_name TEXT NOT NULL, newrecid INTEGER NOT NULL,
            PRIMARY KEY (core_uid, newrecid));
         ALTER TABLE orders_rep ADD COLUMN closedate INTEGER;",
    )
    .unwrap();
    // One transaction for the unchanged 2000-row fixture (the count keeps the damaged leaf off the header page); per-row commits only cost time.
    conn.execute_batch("BEGIN").unwrap();
    {
        let mut stmt = conn
            .prepare(
                "INSERT INTO orders_rep (core_uid, core_name, newrecid, closedate)
                 VALUES (1, 'CORE-A', ?1, ?2)",
            )
            .unwrap();
        for i in 0..2000i64 {
            stmt.execute(rusqlite::params![i, 1_780_000_000i64 + i * 60])
                .unwrap();
        }
    }
    conn.execute_batch("COMMIT").unwrap();
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_t ON orders_rep(closedate)",
        [],
    )
    .unwrap();
    let (pgoffset, pgsize): (i64, i64) = conn
        .query_row(
            "SELECT pgoffset, pgsize FROM dbstat WHERE name='idx_t' AND pagetype='leaf'
             ORDER BY path LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    drop(conn);

    assert_eq!(run(&path), Integrity::Ok, "здоровая БД должна быть Ok");

    let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.seek(SeekFrom::Start(pgoffset as u64)).unwrap();
    f.write_all(&vec![0xFFu8; pgsize as usize]).unwrap();
    f.flush().unwrap();
    drop(f);

    // Page damage is a `Damaged` verdict, distinct from an unavailable check.
    match run(&path) {
        Integrity::Damaged(lines) => {
            assert!(!lines.is_empty(), "должны быть строки диагностики")
        }
        other => panic!("ожидался Damaged, получено {other:?}"),
    }
    assert!(
        matches!(quick(&path), Integrity::Damaged(_)),
        "the cheap startup gate must catch page damage before the first window"
    );

    for suf in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", path.display(), suf));
    }
}

/// `db/integrity/mod.rs:writer_should_stop` must latch corruption until safe recovery.
///
/// Removing the `WRITES_BLOCKED.store` allows the sole writer to retry a malformed image forever
/// and lets later batches continue touching a replica that the background check condemned.
/// Removing `clear_after_recovery`'s active-latch reset leaves the same damage banner and writer
/// block behind after the exact malformed file set was safely retired.
#[test]
fn corruption_latches_the_writer_block() {
    let _state = test_state_guard();
    reset_test_state();
    let error = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::DatabaseCorrupt,
            extended_code: 11,
        },
        None,
    );

    assert!(writer_should_stop(&error));
    assert!(writes_blocked());
    assert!(matches!(active_damage(), Some(Integrity::Damaged(_))));

    clear_after_recovery();
    assert!(!writes_blocked());
    assert!(active_damage().is_none());
    reset_test_state();
}

/// `db/integrity/mod.rs:record_corruption` must publish through the writer ACK barrier.
///
/// Removing the `WRITE_BARRIER.write()` acquisition lets the worker below publish while the
/// simulated ACK section still owns its read guard, so the independent blocked-state assertion
/// turns true too early and the core can advance past a batch after damage became visible.
#[test]
fn corruption_publication_waits_for_the_ack_section() {
    let _state = test_state_guard();
    reset_test_state();
    let ack_guard = writer_ack_guard();
    let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
    let (done_tx, done_rx) = std::sync::mpsc::sync_channel(0);
    let worker = std::thread::spawn(move || {
        let error = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::DatabaseCorrupt,
                extended_code: 11,
            },
            None,
        );
        started_tx.send(()).unwrap();
        record_corruption(&error);
        done_tx.send(()).unwrap();
    });

    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("damage worker must reach the publication barrier");
    assert!(!writes_blocked());
    assert!(matches!(
        done_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    drop(ack_guard);
    done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("damage publication must finish after the ACK section");
    worker.join().unwrap();
    assert!(writes_blocked());
    reset_test_state();
}

/// Measure the full and cheap startup checks over the same synthetic 100000-deal replica.
#[test]
#[ignore = "synthetic startup integrity scale probe"]
fn probe_startup_integrity_100k() {
    let dir = temp_dir("startup-100k");
    let path = dir.join("reports.sqlite");
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
    }
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL, core_name TEXT NOT NULL, newrecid INTEGER NOT NULL,
             closedate INTEGER, strategyid INTEGER, buydate INTEGER, coin TEXT, deleted INTEGER,
             PRIMARY KEY (core_uid, newrecid)
         );
         WITH RECURSIVE ids(n) AS (
             SELECT 1 UNION ALL SELECT n + 1 FROM ids WHERE n < 100000
         )
         INSERT INTO orders_rep
         SELECT (n - 1) % 10 + 1, 'CORE_A', (n - 1) / 10 + 1,
                1700000000 + n, n % 20, 1699990000 + n, 'COIN_A', 0 FROM ids;
         CREATE INDEX idx_rep_closedate ON orders_rep(closedate);
         CREATE INDEX idx_rep_core_close ON orders_rep(core_uid, closedate);
         CREATE INDEX idx_rep_strat ON orders_rep(core_uid, strategyid, buydate);
         CREATE INDEX idx_rep_strategy_close ON orders_rep(core_uid, strategyid, closedate);
         CREATE INDEX idx_rep_open ON orders_rep(core_uid, buydate)
             WHERE NOT (typeof(closedate) IN ('integer','real') AND closedate > 0);
         CREATE INDEX idx_rep_coin_close ON orders_rep(coin COLLATE NOCASE, closedate);
         CREATE INDEX idx_rep_coin_entry ON orders_rep(core_uid, coin COLLATE NOCASE, buydate)
             WHERE buydate > 0;
         PRAGMA wal_checkpoint(TRUNCATE);",
    )
    .unwrap();
    drop(conn);

    let mut full_ms = Vec::new();
    let mut quick_ms = Vec::new();
    for _ in 0..3 {
        let started = Instant::now();
        assert_eq!(run(&path), Integrity::Ok);
        full_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        let started = Instant::now();
        assert_eq!(quick(&path), Integrity::Ok);
        quick_ms.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    full_ms.sort_by(f64::total_cmp);
    quick_ms.sort_by(f64::total_cmp);
    println!(
        "probe_startup_integrity rows=100000 cores=10 full_ms={full_ms:?} quick_ms={quick_ms:?} full_median_ms={:.3} quick_median_ms={:.3}",
        full_ms[1], quick_ms[1]
    );
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
    }
}
