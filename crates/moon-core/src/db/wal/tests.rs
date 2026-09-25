use super::*;

use crate::db::test_support::{remove_db, temp_db};

fn wal_len(path: &std::path::Path) -> u64 {
    std::fs::metadata(format!("{}-wal", path.display()))
        .map(|m| m.len())
        .unwrap_or(0)
}

/// Fill a table with `rows` rows of 4 KiB each, in one transaction.
fn fill(conn: &Connection, rows: i64) {
    let tx = conn.unchecked_transaction().expect("tx");
    for i in 0..rows {
        tx.execute(
            "INSERT INTO t(id, body) VALUES(?1, zeroblob(4096))",
            rusqlite::params![i],
        )
        .expect("insert");
    }
    tx.commit().expect("commit");
}

/// The regression that shipped: a `VACUUM` over a WAL database left a sidecar the size of the
/// database for as long as the connection stayed open.
#[test]
fn a_vacuum_leaves_no_wal_behind() {
    let path = temp_db("wal-vacuum");
    let conn = Connection::open(&path).expect("open");
    enable(&conn).expect("wal");
    conn.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, body BLOB);")
        .expect("table");
    fill(&conn, 2_000);
    conn.execute("DELETE FROM t WHERE id % 2 = 0", [])
        .expect("delete");
    vacuum(&conn).expect("vacuum");
    assert_eq!(
        wal_len(&path),
        0,
        "the checkpoint after the VACUUM cut the file"
    );
    drop(conn);
    remove_db(&path);
}

/// The limit holds without any explicit checkpoint: a transaction larger than the limit grows
/// the sidecar, and the next writer's restart cuts it back.
#[test]
fn a_large_transaction_is_cut_back_to_the_limit() {
    let path = temp_db("wal-limit");
    let conn = Connection::open(&path).expect("open");
    enable(&conn).expect("wal");
    // A limit of the test's own, small enough to cross with a few megabytes.
    let limit: i64 = 256 * 1024;
    conn.pragma_update(None, "journal_size_limit", limit)
        .expect("limit");
    conn.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, body BLOB);")
        .expect("table");
    fill(&conn, 1_000);
    assert!(wal_len(&path) > limit as u64, "the batch grew the sidecar");
    let _ = conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()));
    // The restart happens on the next write after a complete checkpoint.
    conn.execute("INSERT INTO t(id, body) VALUES(-1, x'00')", [])
        .expect("write");
    assert!(
        wal_len(&path) <= limit as u64,
        "cut back to the limit: {} bytes",
        wal_len(&path)
    );
    drop(conn);
    remove_db(&path);
}
