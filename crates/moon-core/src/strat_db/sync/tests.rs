//! The sync's reads and its packing: what crosses the wire and how it pages.

use serde_json::json;

use super::super::write::init;
use super::*;
use crate::config::storage::StrategiesStoreCfg;

fn setup() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    let cfg = StrategiesStoreCfg {
        ignore_fields: Vec::new(),
    };
    init(&conn, &cfg).unwrap();
    conn
}

/// File `count` versions of a strategy, each with `filler` bytes of a text that does not compress.
fn file_versions(conn: &Connection, core: i64, strategy: i64, count: i64, filler: usize) {
    conn.execute(
        "INSERT OR IGNORE INTO strategies (core_uid, strategy_id, name, kind) VALUES (?1, ?2, 'S', 'K')",
        params![core, strategy],
    )
    .unwrap();
    for n in 0..count {
        // Xorshift: a multiplicative pattern is periodic, and gzip then folds the whole chain.
        let mut state = (n as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let noise: String = (0..filler)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                char::from(b'a' + (state >> 59) as u8 % 26)
            })
            .collect();
        conn.execute(
            "INSERT INTO strategy_versions (core_uid, strategy_id, valid_from, change_kind, raw_json)
             VALUES (?1, ?2, ?3, 'params', ?4)",
            params![core, strategy, 1000 + n, json!({"N": n, "X": noise}).to_string()],
        )
        .unwrap();
    }
}

/// Regression target: a listing that repeats or skips across its pages would make the terminal
/// fetch forever or miss a strategy.
#[test]
fn the_index_pages_without_a_gap_or_a_repeat() {
    let conn = setup();
    for strategy in 0..5 {
        file_versions(&conn, 1, strategy - 2, 2, 1);
    }
    file_versions(&conn, 2, 7, 3, 1);
    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let page = index_on(&conn, after.as_ref(), SUMMARY_JSON * 2).unwrap();
        assert!(page.strategies.len() <= 2);
        seen.extend(page.strategies);
        match page.resume {
            Some(resume) => after = Some(resume),
            None => break,
        }
    }
    let keys: Vec<(u64, i64)> = seen.iter().map(|s| (s.core_uid, s.strategy_id)).collect();
    assert_eq!(
        keys,
        [(1, -2), (1, -1), (1, 0), (1, 1), (1, 2), (2, 7)],
        "negative ids included: a strategy id above 2^63 is stored negative"
    );
    assert_eq!(seen[5].versions, 3);
    assert_eq!((seen[5].first_from, seen[5].last_from), (1000, 1002));
}

/// A chain that does not fit one frame arrives whole over several, in order.
#[test]
fn a_long_chain_comes_whole_over_pages() {
    let conn = setup();
    file_versions(&conn, 1, 5, 40, 4_000);
    let budget = 32 * 1024;
    let mut versions = Vec::new();
    let mut after = None;
    let mut pages = 0;
    loop {
        let page = chain_page_on(&conn, 1, 5, after, budget).unwrap();
        assert!(page.versions.len() <= budget);
        assert_eq!(
            page.head.is_some(),
            pages == 0,
            "the first page has the head"
        );
        versions.extend(unpack(&page.versions).unwrap());
        pages += 1;
        match page.resume {
            Some(resume) => after = Some(resume),
            None => break,
        }
    }
    assert!(pages > 1, "the test needs a chain over one frame");
    let times: Vec<i64> = versions.iter().map(|v| v.valid_from).collect();
    assert_eq!(times, (1000..1040).collect::<Vec<_>>());
}

/// A single version over the frame stops the exchange of that chain rather than looping on it.
#[test]
fn a_version_over_the_frame_is_an_error() {
    let conn = setup();
    file_versions(&conn, 1, 5, 1, 4_000);
    assert!(chain_page_on(&conn, 1, 5, None, 64).is_err());
}

/// Packing is the wire format: both ends must read each other.
#[test]
fn packed_versions_come_back_the_same() {
    let conn = setup();
    file_versions(&conn, 1, 5, 3, 10);
    let page = chain_page_on(&conn, 1, 5, None, 1 << 20).unwrap();
    let versions = unpack(&page.versions).unwrap();
    assert_eq!(unpack(&pack(&versions).unwrap()).unwrap(), versions);
}

/// Regression target: unpacking without a limit. A small page of zeros would fill a small server.
#[test]
fn a_page_that_unpacks_over_the_limit_is_refused() {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    let block = vec![b' '; 1 << 20];
    for _ in 0..=(PAGE_UNPACKED_LIMIT >> 20) {
        encoder.write_all(&block).unwrap();
    }
    let packed = STANDARD.encode(encoder.finish().unwrap());
    assert!(packed.len() < 1 << 20, "small on the wire");
    let error = unpack(&packed).unwrap_err().to_string();
    assert!(error.contains("over"), "{error}");
}

/// Garbage is an error, not a panic on the API's thread.
#[test]
fn garbage_is_an_error() {
    assert!(unpack("not base64!").is_err());
    assert!(unpack(&STANDARD.encode(b"not gzip")).is_err());
}
