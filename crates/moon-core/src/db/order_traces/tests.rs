use std::path::PathBuf;

use rusqlite::Connection;

use super::*;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Fresh on-disk store for one test; the backfill query attaches it by path, so `:memory:` is
/// not enough for that half.
fn store(tag: &str) -> (PathBuf, Connection) {
    let dir = std::env::temp_dir().join(format!("moon-order-traces-{tag}-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("order_traces.sqlite");
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let conn = Connection::open(&path).unwrap();
    init(&conn).unwrap();
    (path, conn)
}

fn line(own: bool, kind: ArchivedLineKind, points: &[(f64, f64)]) -> ArchivedOrderTrace {
    ArchivedOrderTrace {
        own,
        kind,
        stop_price: None,
        stop_time_ms: None,
        points: points.to_vec(),
    }
}

#[test]
fn points_round_trip_at_full_width() {
    let points = vec![
        (1_700_000_000_123.0, 0.123456789012345),
        (1_700_000_060_000.0, 71234.5),
    ];
    assert_eq!(decode_points(&encode_points(&points)), points);
    // A torn tail is dropped, not read as a point at the epoch.
    let mut torn = encode_points(&points);
    torn.truncate(20);
    assert_eq!(decode_points(&torn), points[..1]);
}

#[test]
fn answer_with_lines_reads_back_verbatim_and_replaces_an_older_one() {
    let (_path, conn) = store("lines");
    let first = [line(
        true,
        ArchivedLineKind::Entry,
        &[(1_000.0, 1.0), (2_000.0, 1.1)],
    )];
    store_answer(&conn, 5, -42, &first, 10).unwrap();
    let mut second = vec![
        line(true, ArchivedLineKind::Entry, &[(1_000.0, 1.0)]),
        line(false, ArchivedLineKind::Exit, &[(3_000.0, 2.0)]),
    ];
    second[1].stop_price = Some(1.95);
    second[1].stop_time_ms = Some(3_500.0);
    store_answer(&conn, 5, -42, &second, 20).unwrap();

    let read = read_many_on(&conn, 5, &[-42, 0, 7]).unwrap();
    assert_eq!(read.len(), 1, "unknown and zero uids are simply absent");
    let TraceEntry::Lines(lines) = &read[&-42] else {
        panic!("expected lines");
    };
    assert_eq!(&lines[..], &second[..]);
    // Another core's rows are another key.
    assert!(read_many_on(&conn, 6, &[-42]).unwrap().is_empty());
}

#[test]
fn empty_answer_is_filed_with_its_date_and_ages_out() {
    let (_path, conn) = store("empty");
    store_answer(&conn, 1, 9, &[], 1_000).unwrap();
    let read = read_many_on(&conn, 1, &[9]).unwrap();
    assert_eq!(
        read[&9],
        TraceEntry::Empty {
            checked_at_ms: 1_000
        }
    );
    assert!(read[&9].is_current(1_000 + 6 * DAY_MS));
    assert!(!read[&9].is_current(1_000 + 8 * DAY_MS));
    // The close hook asks only once the empty answer is stale; lines are never re-asked.
    assert!(!needs_ask(&conn, 1, 9, 1_000 + DAY_MS).unwrap());
    assert!(needs_ask(&conn, 1, 9, 1_000 + 8 * DAY_MS).unwrap());
    assert!(needs_ask(&conn, 1, 10, 1_000).unwrap(), "nothing filed");
    store_answer(
        &conn,
        1,
        9,
        &[line(true, ArchivedLineKind::Entry, &[(1.0, 1.0)])],
        2_000,
    )
    .unwrap();
    assert!(!needs_ask(&conn, 1, 9, 1_000 + 100 * DAY_MS).unwrap());
}

#[test]
fn schema_version_from_the_future_is_refused() {
    let (_path, conn) = store("version");
    conn.execute(
        "UPDATE app_meta SET value='99' WHERE key='schema_version'",
        [],
    )
    .unwrap();
    assert!(init(&conn).is_err());
}

/// Replica with `reportuid`, `closedate` and `deleted`; rows are `(newrecid, reportuid,
/// closedate_secs, deleted)`, all on core 1.
fn replica(traces_path: &std::path::Path, rows: &[(i64, i64, i64, i64)]) -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER NOT NULL, core_name TEXT NOT NULL,
             newrecid INTEGER NOT NULL, reportuid INTEGER, closedate INTEGER, deleted INTEGER,
             PRIMARY KEY (core_uid, newrecid));",
    )
    .unwrap();
    for (rec, uid, close, deleted) in rows {
        conn.execute(
            "INSERT INTO orders_rep VALUES (1, 'A', ?1, ?2, ?3, ?4)",
            rusqlite::params![rec, uid, close, deleted],
        )
        .unwrap();
    }
    conn.execute(
        "ATTACH DATABASE ?1 AS traces",
        [traces_path.to_string_lossy().as_ref()],
    )
    .unwrap();
    conn
}

#[test]
fn backfill_lists_recent_closed_unknown_rows_newest_first() {
    let (path, writer) = store("backfill");
    let now_ms = 40 * DAY_MS;
    let now_s = now_ms / 1000;
    // Filed with lines: never listed. Filed empty yesterday: not listed. Filed empty long ago:
    // listed again.
    store_answer(
        &writer,
        1,
        100,
        &[line(true, ArchivedLineKind::Entry, &[(1.0, 1.0)])],
        0,
    )
    .unwrap();
    store_answer(&writer, 1, 101, &[], now_ms - DAY_MS).unwrap();
    store_answer(&writer, 1, 102, &[], now_ms - 10 * DAY_MS).unwrap();
    let day_s = DAY_MS / 1000;
    let conn = replica(
        &path,
        &[
            (1, 100, now_s - day_s, 0),
            (2, 101, now_s - day_s, 0),
            (3, 102, now_s - 2 * day_s, 0),
            (4, 103, now_s - 3 * day_s, 0),
            (5, 104, now_s - day_s, 0),
            (6, 105, now_s - day_s, 1),
            (7, 0, now_s - day_s, 0),
            (8, 106, 0, 0),
            (9, 107, now_s - 35 * day_s, 0),
        ],
    );
    let uids = backfill_candidates_on(&conn, 1, now_ms).unwrap();
    // 104 and 103 are unknown; 102's empty answer is stale; 105 is soft-deleted, 0 is not a key,
    // 106 is open, 107 is past the depth even with the clock slack.
    assert_eq!(uids, vec![104, 102, 103]);
    assert!(backfill_candidates_on(&conn, 2, now_ms).unwrap().is_empty());
}

#[test]
fn backfill_without_the_uid_column_is_not_ready() {
    let (path, _writer) = store("nocol");
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER NOT NULL, core_name TEXT NOT NULL,
             newrecid INTEGER NOT NULL, closedate INTEGER, PRIMARY KEY (core_uid, newrecid));",
    )
    .unwrap();
    conn.execute(
        "ATTACH DATABASE ?1 AS traces",
        [path.to_string_lossy().as_ref()],
    )
    .unwrap();
    assert!(matches!(
        backfill_candidates_on(&conn, 1, DAY_MS),
        Err(ReadFail::NotReady)
    ));
}
