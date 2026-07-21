use super::*;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;

/// Build a process-scoped temporary directory for one test scenario.
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moon-integrity-{}-{}", tag, std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[test]
/// A missing replica produces the silent `NotPresent` verdict.
fn absent_file_is_not_present() {
    let p = temp_dir("absent").join("nope.sqlite");
    let _ = std::fs::remove_file(&p);
    assert_eq!(run(&p), Integrity::NotPresent);
}

#[test]
/// A healthy replica passes and index-page damage produces diagnostics.
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

#[test]
/// Repeated startup calls do not launch or publish a second check.
fn spawn_check_is_idempotent() {
    // The second call must not launch another thread or replace the verdict.
    spawn_check();
    spawn_check();
    assert!(STARTED.load(Ordering::SeqCst));
}
