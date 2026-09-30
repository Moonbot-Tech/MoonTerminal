//! The tape between the station and a terminal: the wire packing and the read-only reader.

use super::super::{init_schema, insert_span};
use super::*;
use crate::feed::types::Side;

fn tick(time_ms: i64, price: f32, side: Side) -> Tick {
    Tick {
        time_ms: time_ms as f64,
        price,
        qty: 0.5,
        side,
    }
}

/// What crosses the wire comes back as filed: whole-millisecond stamps, prices, sides.
#[test]
fn prints_survive_the_wire() {
    let ticks = vec![
        tick(1_000, 1.25, Side::Buy),
        tick(1_000, 1.5, Side::Sell),
        tick(1_750, 1.0, Side::Buy),
    ];
    let back = decode_prints(&encode_prints(&ticks)).expect("decodes");
    assert_eq!(back.len(), 3);
    for (a, b) in ticks.iter().zip(&back) {
        assert_eq!(
            (a.time_ms, a.price, a.qty, a.side),
            (b.time_ms, b.price, b.qty, b.side)
        );
    }
    assert!(decode_prints("not base64!").is_none());
    assert_eq!(decode_prints(&encode_prints(&[])).map(|t| t.len()), Some(0));
}

/// The station reads the recorder's file without owning it: spans and quiet stretches come back,
/// and a row that does not decode is skipped, not deleted.
#[test]
fn a_tape_file_reads_without_owning() {
    let path = std::env::temp_dir().join(format!(
        "moon-tape-file-{}-{}.sqlite",
        std::process::id(),
        crate::util::time::now_unix_ms_i64()
    ));
    {
        let conn = rusqlite::Connection::open(&path).expect("file");
        init_schema(&conn).expect("schema");
        insert_span(
            &conn,
            "x",
            "M",
            0,
            99,
            &[tick(50, 2.0, Side::Buy)],
            TileSource::Core,
            1,
        )
        .expect("span");
        insert_span(&conn, "x", "M", 100, 199, &[], TileSource::Core, 1).expect("quiet");
        conn.execute(
            "INSERT INTO packs(exchange, market, from_ms, to_ms, ticks, prints, source, updated_ms)
             VALUES('x', 'M', 300, 399, x'FF', 1, 1, 1)",
            [],
        )
        .expect("broken row");
    }
    let file = TapeFile::open(&path).expect("read-only open");
    let spans = file.spans("x", "M", 0, 1_000).expect("read");
    assert_eq!(
        spans
            .iter()
            .map(|s| (s.from_ms, s.to_ms, s.ticks.len()))
            .collect::<Vec<_>>(),
        vec![(0, 99, 1), (100, 199, 0)]
    );
    drop(file);
    let rows: i64 = rusqlite::Connection::open(&path)
        .expect("file")
        .query_row("SELECT COUNT(*) FROM packs", [], |r| r.get(0))
        .expect("count");
    assert_eq!(
        rows, 3,
        "a reader that does not own the file deletes nothing"
    );
    let _ = std::fs::remove_file(&path);
}
