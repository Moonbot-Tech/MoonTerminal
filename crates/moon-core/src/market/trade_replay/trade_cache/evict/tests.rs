use super::super::{TradeCache, held_bytes, init_schema, insert_span, read_span_bounds};
use super::*;
use crate::feed::types::{Side, Tick};
use crate::market::trade_replay::tick_tiles::TileSource;

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("moon-evict-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// A file on disk, not in memory: what is measured is the file's length.
fn file_conn(name: &str) -> (Connection, std::path::PathBuf) {
    let path = temp_dir(name).join("tape.sqlite");
    let conn = Connection::open(&path).expect("open");
    crate::db::wal::enable(&conn).expect("wal");
    init_schema(&conn).expect("schema");
    (conn, path)
}

/// Prints that do not pack small: every one a different price and size.
fn prints(from_ms: i64, count: i64) -> Vec<Tick> {
    (0..count)
        .map(|i| Tick {
            time_ms: (from_ms + i) as f64,
            price: 1.0 + (i * 7919 % 10_007) as f32 / 3.0,
            qty: (i * 104_729 % 1_000_003) as f32 / 7.0,
            side: if i % 2 == 0 { Side::Buy } else { Side::Sell },
        })
        .collect()
}

/// Four spans a megabyte of time apart, filed at 1..4 s.
fn four_spans(conn: &Connection) {
    for (i, updated_ms) in [1_000, 2_000, 3_000, 4_000].into_iter().enumerate() {
        let from_ms = i as i64 * 1_000_000;
        insert_span(
            conn,
            "x",
            "M",
            from_ms,
            from_ms + 99_999,
            &prints(from_ms, 50_000),
            TileSource::Core,
            updated_ms,
        )
        .expect("insert");
    }
}

fn file_len(conn: &Connection, path: &std::path::Path) -> u64 {
    let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    std::fs::metadata(path).expect("metadata").len()
}

/// The spans filed longest ago go first: one byte asked takes the oldest span alone.
#[test]
fn eviction_takes_the_oldest_first() {
    let (conn, path) = file_conn("oldest");
    make_shrinkable(&conn).expect("shrinkable");
    four_spans(&conn);
    let held = held_bytes(&conn).expect("held");
    let Evicted::Freed { held: after, .. } = evict_oldest(&conn, held, 1).expect("evict") else {
        panic!("a shrinkable file evicts");
    };
    assert!(after < held);
    let left = read_span_bounds(&conn, "x", "M", 0, i64::MAX).expect("bounds");
    assert_eq!(
        left,
        vec![
            (1_000_000, 1_099_999),
            (2_000_000, 2_099_999),
            (3_000_000, 3_099_999)
        ]
    );
    drop(conn);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// The file itself gets shorter by what the spans held — a disk short of space sees the room
/// come back. More asked than the file holds empties it, and an empty file evicts nothing more.
#[test]
fn eviction_shrinks_the_file() {
    let (conn, path) = file_conn("shrink");
    make_shrinkable(&conn).expect("shrinkable");
    four_spans(&conn);
    let held = held_bytes(&conn).expect("held");
    let before = file_len(&conn, &path);

    let evicted = evict_oldest(&conn, held, held * 10).expect("evict");
    assert_eq!(
        evicted,
        Evicted::Freed {
            held: 0,
            shrunk: true
        }
    );
    let shrunk = file_len(&conn, &path);
    assert!(
        shrunk + (held / 2) as u64 <= before,
        "the file kept its length: {before} → {shrunk} bytes after evicting {held} bytes"
    );
    assert!(
        read_span_bounds(&conn, "x", "M", 0, i64::MAX)
            .expect("bounds")
            .is_empty()
    );
    assert!(matches!(
        evict_oldest(&conn, 0, 1_000).expect("evict empty"),
        Evicted::Freed { held: 0, .. }
    ));
    drop(conn);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// A file made without the mode (its switch at open failed) deletes nothing: rows gone there
/// would free no disk and only lose the tape.
#[test]
fn a_file_without_the_mode_evicts_nothing() {
    let (conn, path) = file_conn("switch");
    four_spans(&conn);
    let held = held_bytes(&conn).expect("held");
    assert_eq!(
        evict_oldest(&conn, held, held).expect("evict"),
        Evicted::NotShrinkable
    );
    assert_eq!(held_bytes(&conn).expect("held"), held);
    drop(conn);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// The worker's own path: a file opened evictable takes an eviction through the queue, and what
/// it holds afterwards is what the read says.
#[test]
fn the_worker_evicts_through_its_queue() {
    let path = temp_dir("worker").join("tape.sqlite");
    let cache = TradeCache::open_evictable(path.clone()).expect("cache");
    for i in 0..4i64 {
        let from_ms = i * 1_000_000;
        cache.insert(
            "x",
            "M",
            from_ms,
            from_ms + 99_999,
            prints(from_ms, 20_000),
            TileSource::Core,
        );
        // Distinct write stamps: the eviction order is the order the spans were filed in.
        assert!(cache.sync(std::time::Duration::from_secs(10)));
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    cache.evict_oldest(1);
    assert!(cache.sync(std::time::Duration::from_secs(10)));
    let held = cache.held_spans("x", "M", 0, i64::MAX).expect("spans");
    assert_eq!(
        held,
        vec![
            (1_000_000, 1_099_999),
            (2_000_000, 2_099_999),
            (3_000_000, 3_099_999)
        ]
    );
    drop(cache);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
