use super::*;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;

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

    for suf in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", path.display(), suf));
    }
}

/// Repeated startup calls do not launch or publish a second check.
#[test]
fn spawn_check_is_idempotent() {
    // The second call must not launch another thread or replace the verdict.
    spawn_check();
    spawn_check();
    assert!(STARTED.load(Ordering::SeqCst));
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
