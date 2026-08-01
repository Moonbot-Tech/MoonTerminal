//! Shared fixtures for the reports-replica regression tests.
//!
//! Both the analytics and report read paths must propagate a damaged b-tree
//! page reported by SQLite. Building that fixture takes a real on-disk database
//! because an in-memory database has no pages to corrupt, so the paths share it.

use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use rusqlite::Connection;

/// Build a process-scoped temporary directory; callers remove it explicitly.
///
/// `tag` separates concurrently running tests, which share one process.
pub(super) fn temp_db(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moon-analytics-{}-{}", tag, std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("reports-test.sqlite");
    remove_db(&path);
    path
}

/// Remove a temporary SQLite database and its WAL sidecars.
pub(super) fn remove_db(path: &Path) {
    for suf in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", path.display(), suf));
    }
}

/// Run `rep::init` with throwaway cursor and open-row registries.
///
/// Tests that do not exercise catch-up bookkeeping share this one-liner; those
/// that do feed the returned state to `rep::apply_*`.
pub(super) fn rep_init(conn: &Connection) -> super::rep::RepState {
    let cursors = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let open_rows = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    super::rep::init(conn, cursors, open_rows).unwrap()
}

/// Build a replica whose columns predate `rep::init`, allowing index creation
/// to follow the same path as an upgraded database.
///
/// Each row is `(closedate, profitbtc, coin)`; `buydate` trails `closedate` by
/// five minutes and every row belongs to core 1, strategy 7, non-emulator.
///
/// Args:
///     path: Temporary SQLite database path.
///     rows: Independent close-time, profit, and market fixture rows.
///
/// Returns:
///     Open seeded replica connection with quote identity set to USDT.
pub(super) fn build_replica(path: &Path, rows: &[(i64, f64, &str)]) -> Connection {
    let conn = Connection::open(path).unwrap();
    super::init_db(&conn).unwrap();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS orders_rep (core_uid INTEGER NOT NULL,
                core_name TEXT NOT NULL, newrecid INTEGER NOT NULL,
                PRIMARY KEY (core_uid, newrecid));
             ALTER TABLE orders_rep ADD COLUMN closedate INTEGER;
             ALTER TABLE orders_rep ADD COLUMN buydate INTEGER;
             ALTER TABLE orders_rep ADD COLUMN profitbtc REAL;
             ALTER TABLE orders_rep ADD COLUMN spentbtc REAL;
             ALTER TABLE orders_rep ADD COLUMN coin TEXT;
             ALTER TABLE orders_rep ADD COLUMN strategyid INTEGER;
             ALTER TABLE orders_rep ADD COLUMN isshort INTEGER;
             ALTER TABLE orders_rep ADD COLUMN emulator INTEGER;
             ALTER TABLE orders_rep ADD COLUMN deleted INTEGER;
             ALTER TABLE orders_rep ADD COLUMN basecurrency INTEGER;",
    )
    .unwrap();
    rep_init(&conn);

    let mut stmt = conn
        .prepare(
            "INSERT INTO orders_rep (core_uid, core_name, newrecid, closedate, buydate,
                    profitbtc, spentbtc, coin, strategyid, isshort, emulator, deleted,
                    basecurrency)
                 VALUES (1, 'CORE-A', ?1, ?2, ?2 - 300, ?3, 100.0, ?4, 7, 0, 0, 0, 1)",
        )
        .unwrap();
    for (i, (close, profit, coin)) in rows.iter().enumerate() {
        stmt.execute(rusqlite::params![i as i64 + 1, close, profit, coin])
            .unwrap();
    }
    drop(stmt);
    conn
}

/// Rows spread one minute apart from `day`, two of every three at a loss.
///
/// The count matters: enough pages must exist for the damaged leaf to sit away
/// from the header, so `Connection::open` succeeds and the failure occurs when
/// a query reaches that leaf.
pub(super) fn spread_rows(day: i64, n: i64) -> Vec<(i64, f64, &'static str)> {
    (0..n)
        .map(|i| (day + i * 60, if i % 3 == 0 { 5.0 } else { -1.0 }, "BTCUSDT"))
        .collect()
}

/// Overwrite the first leaf page of `name` (a table or an index) with 0xFF.
///
/// Consumes `conn`: the WAL is checkpointed and the connection closed first,
/// otherwise SQLite would serve the undamaged page from the WAL and the test
/// would silently prove nothing. The exact page comes from `dbstat` rather than
/// a guess, so the damage is reproducible across SQLite versions.
pub(super) fn corrupt_leaf_page(conn: Connection, path: &Path, name: &str) {
    let (pgoffset, pgsize): (i64, i64) = conn
        .query_row(
            "SELECT pgoffset, pgsize FROM dbstat
             WHERE name = ?1 AND pagetype = 'leaf'
             ORDER BY path LIMIT 1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("dbstat доступен и у объекта есть листовые страницы");
    assert!(pgoffset > 0 && pgsize > 0);

    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    drop(conn);

    let mut f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.seek(SeekFrom::Start(pgoffset as u64)).unwrap();
    f.write_all(&vec![0xFFu8; pgsize as usize]).unwrap();
    f.flush().unwrap();
}
