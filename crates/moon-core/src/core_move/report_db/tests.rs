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
    let summary = build(&src, 7, &out).unwrap().expect("core 7 has rows");
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
    let summary = build(&src, 7, &out).unwrap().expect("core 7 has rows");
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
    assert_eq!(build(&src, 42, &other).unwrap(), None, "no rows, no file");
    assert!(!other.exists(), "a core without rows gets no file");
}

/// The `ID` of every `Orders` row, in `ID` order, with its coin.
fn ids_and_coins(out: &Path) -> Vec<(i64, String)> {
    Connection::open(out)
        .unwrap()
        .prepare("SELECT ID, coin FROM Orders ORDER BY ID")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Most cores never send their local row id, so the replica the writer builds has no `id` column
/// at all (#1186): the rebuild keys `Orders` by `newRecID` instead of refusing.
#[test]
fn a_replica_without_an_id_column_keys_orders_by_newrecid() {
    let dir = scratch("noid");
    let src = dir.join("reports.sqlite");
    Connection::open(&src)
        .unwrap()
        .execute_batch(
            "CREATE TABLE app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE orders_rep (core_uid INTEGER NOT NULL, core_name TEXT NOT NULL,
                 newrecid INTEGER NOT NULL, PRIMARY KEY (core_uid, newrecid));
             ALTER TABLE orders_rep ADD COLUMN coin TEXT;
             ALTER TABLE orders_rep ADD COLUMN closedate INTEGER;
             INSERT INTO orders_rep VALUES (7, 'BB1', 3, 'ETH', 200);
             INSERT INTO orders_rep VALUES (7, 'BB1', 5, 'BTC', 0);
             ALTER TABLE orders_rep ADD COLUMN buydate INTEGER;",
        )
        .unwrap();
    let out = dir.join("core.db");
    let summary = build(&src, 7, &out).unwrap().expect("core 7 has rows");
    assert_eq!(
        (summary.rows, summary.open_rows, summary.high_water),
        (2, 1, 5)
    );
    assert_eq!(
        ids_and_coins(&out),
        [(3, "ETH".to_owned()), (5, "BTC".to_owned())]
    );
    let rec_ids: Vec<i64> = Connection::open(&out)
        .unwrap()
        .prepare("SELECT newRecID FROM Orders ORDER BY ID")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rec_ids, [3, 5], "newRecID is carried as well");
}

/// Another core that does send `ID` adds the column for everybody, and a core that started sending
/// it after an update fills it only on newer rows. A partly filled id would collide with the
/// `newRecID` minted for the rest, so the whole core falls back to `newRecID`.
#[test]
fn an_id_column_this_core_left_partly_empty_falls_back_to_newrecid() {
    let dir = scratch("partid");
    let src = dir.join("reports.sqlite");
    Connection::open(&src)
        .unwrap()
        .execute_batch(
            "CREATE TABLE app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE orders_rep (core_uid INTEGER NOT NULL, core_name TEXT NOT NULL,
                 newrecid INTEGER NOT NULL, coin TEXT, id INTEGER,
                 PRIMARY KEY (core_uid, newrecid));
             INSERT INTO orders_rep VALUES (7, 'BB1', 10, 'ETH', NULL);
             INSERT INTO orders_rep VALUES (7, 'BB1', 11, 'BTC', NULL);
             INSERT INTO orders_rep VALUES (7, 'BB1', 12, 'SOL', 10);
             INSERT INTO orders_rep VALUES (9, 'Aster', 1, 'XRP', 44);
             INSERT INTO orders_rep VALUES (11, 'Odd', 4, 'ADA', 'x4');
             ALTER TABLE orders_rep ADD COLUMN buydate INTEGER;
             ALTER TABLE orders_rep ADD COLUMN closedate INTEGER;",
        )
        .unwrap();
    let out = dir.join("core.db");
    build(&src, 7, &out).unwrap().expect("core 7 has rows");
    assert_eq!(
        ids_and_coins(&out),
        [
            (10, "ETH".to_owned()),
            (11, "BTC".to_owned()),
            (12, "SOL".to_owned())
        ]
    );

    let aster = dir.join("aster.db");
    build(&src, 9, &aster).unwrap().expect("core 9 has rows");
    assert_eq!(
        ids_and_coins(&aster),
        [(44, "XRP".to_owned())],
        "a core that filled every id keeps its own"
    );

    let odd = dir.join("odd.db");
    build(&src, 11, &odd).unwrap().expect("core 11 has rows");
    assert_eq!(
        ids_and_coins(&odd),
        [(4, "ADA".to_owned())],
        "an id that is no integer cannot key Orders"
    );
}

/// A replica the writer never got to create a table in holds no rows of any core.
#[test]
fn a_replica_without_the_table_gives_no_file() {
    let dir = scratch("notable");
    let src = dir.join("reports.sqlite");
    Connection::open(&src)
        .unwrap()
        .execute_batch("CREATE TABLE app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
        .unwrap();
    let out = dir.join("core.db");
    assert_eq!(build(&src, 7, &out).unwrap(), None);
    assert!(!out.exists());
}
