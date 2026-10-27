use std::path::PathBuf;

use rusqlite::Connection;

use super::*;

/// A fresh scratch directory for one test.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "moon-core-move-{tag}-{}-{}",
        std::process::id(),
        crate::util::time::now_unix_ms_i64()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A replica with two cores: core 7 (three rows, one open) and core 9 (one row).
fn replica(dir: &Path, epoch_of_7: Option<i64>, next_of_7: Option<i64>) -> PathBuf {
    let path = dir.join("reports.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TABLE app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE orders_rep (core_uid INTEGER NOT NULL, core_name TEXT NOT NULL,
             newrecid INTEGER NOT NULL, id INTEGER, coin TEXT, buydate INTEGER,
             closedate INTEGER, profitbtc REAL, deleted INTEGER,
             PRIMARY KEY (core_uid, newrecid));
         INSERT INTO orders_rep VALUES (7, 'BB1', 10, 10, 'ETH', 100, 200, 1.5, 0);
         INSERT INTO orders_rep VALUES (7, 'BB1', 11, 11, 'BTC', 110, 0, NULL, 0);
         INSERT INTO orders_rep VALUES (7, 'BB1', 12, 12, 'SOL', 120, 220, -0.5, 1);
         INSERT INTO orders_rep VALUES (9, 'Gate', 10, 10, 'XRP', 100, 200, 9.0, 0);",
    )
    .unwrap();
    if let Some(epoch) = epoch_of_7 {
        db.execute(
            "INSERT INTO app_meta VALUES ('rep_epoch_7', ?1)",
            [epoch.to_string()],
        )
        .unwrap();
    }
    if let Some(next) = next_of_7 {
        db.execute(
            "INSERT INTO app_meta VALUES ('rep_next_7', ?1)",
            [next.to_string()],
        )
        .unwrap();
    }
    path
}

#[test]
fn the_rebuilt_file_carries_the_epoch_the_high_water_and_only_that_cores_rows() {
    let dir = scratch("full");
    let src = replica(&dir, Some(-329_991_539), Some(40));
    let out = dir.join("ByBit Futures.db");
    let summary = build(&src, 7, &out).unwrap();
    assert_eq!(
        summary,
        CoreDbSummary {
            rows: 3,
            open_rows: 1,
            epoch: Some(-329_991_539),
            // The checkpoint cursor (40) is above the last row the replica holds (12): rows the
            // core removed still count, so the core must not mint those ids again.
            high_water: 39,
        }
    );

    let db = Connection::open(&out).unwrap();
    let app_id: i64 = db
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .unwrap();
    let user_version: i64 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(app_id, -329_991_539);
    assert_eq!(user_version, 39);
    let coins: Vec<String> = db
        .prepare("SELECT coin FROM Orders ORDER BY ID")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(coins, ["ETH", "BTC", "SOL"]);
    let deleted: i64 = db
        .query_row("SELECT deleted FROM Orders WHERE ID=12", [], |r| r.get(0))
        .unwrap();
    assert_eq!(deleted, 1, "a hidden row stays hidden");
    let cols: Vec<String> = db
        .prepare("PRAGMA table_info(Orders)")
        .unwrap()
        .query_map([], |r| r.get(1))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(!cols.iter().any(|c| c == "core_uid" || c == "core_name"));
    let dbver: i64 = db
        .query_row("SELECT dbver FROM MainInfo", [], |r| r.get(0))
        .unwrap();
    assert_eq!(dbver, CORE_DB_VERSION);
    let pk: i64 = db
        .query_row(
            "SELECT pk FROM pragma_table_info('Orders') WHERE name='ID'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pk, 1, "Orders is keyed by ID");
}

#[test]
fn without_a_stored_epoch_the_file_carries_none() {
    let dir = scratch("noepoch");
    let src = replica(&dir, None, None);
    let out = dir.join("core.db");
    let summary = build(&src, 7, &out).unwrap();
    assert_eq!(summary.epoch, None);
    assert_eq!(summary.high_water, 12);
    let db = Connection::open(&out).unwrap();
    let app_id: i64 = db
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .unwrap();
    assert_eq!(app_id, 0);
}

#[test]
fn an_existing_file_is_never_overwritten_and_an_unknown_core_leaves_nothing() {
    let dir = scratch("refuse");
    let src = replica(&dir, Some(5), Some(13));
    let out = dir.join("core.db");
    std::fs::write(&out, b"keep me").unwrap();
    assert!(build(&src, 7, &out).is_err());
    assert_eq!(std::fs::read(&out).unwrap(), b"keep me");

    let other = dir.join("other.db");
    assert!(build(&src, 42, &other).is_err());
    assert!(!other.exists(), "a failed rebuild removes its file");
}
