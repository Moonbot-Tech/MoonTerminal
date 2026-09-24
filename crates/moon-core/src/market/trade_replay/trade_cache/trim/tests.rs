use super::super::{StoredSpan, init_schema, insert_span, read_spans};
use super::*;
use crate::feed::types::{Side, Tick};
use crate::market::trade_replay::tick_tiles::TileSource;

fn conn() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
    init_schema(&conn).expect("schema");
    conn
}

fn tick(time_ms: i64) -> Tick {
    Tick {
        time_ms: time_ms as f64,
        price: 1.0,
        qty: 1.0,
        side: Side::Buy,
    }
}

/// Prints every `step` ms from `from` to `to`, both inclusive.
fn ticks(from: i64, to: i64, step: i64) -> Vec<Tick> {
    (from..=to).step_by(step as usize).map(tick).collect()
}

fn file(conn: &rusqlite::Connection, key: (&str, &str), from: i64, to: i64, prints: &[Tick]) {
    insert_span(conn, key.0, key.1, from, to, prints, TileSource::Core, 7).expect("insert");
}

/// One market of the map with the stretches to keep on it.
type KeepEntry<'a> = ((&'a str, &'a str), &'a [(i64, i64)]);

fn keep(entries: &[KeepEntry<'_>]) -> KeepMap {
    entries
        .iter()
        .map(|((exchange, market), spans)| {
            (
                (exchange.to_string(), market.to_string()),
                Coverage::from_spans(spans.iter().copied()),
            )
        })
        .collect()
}

fn bounds(spans: &[StoredSpan]) -> Vec<(i64, i64, usize)> {
    spans
        .iter()
        .map(|s| (s.from_ms, s.to_ms, s.ticks.len()))
        .collect()
}

/// A span the map covers whole is neither counted nor touched.
#[test]
fn a_span_inside_the_map_stays() {
    let conn = conn();
    file(&conn, ("x", "M"), 100, 199, &ticks(100, 199, 10));
    let map = keep(&[(("x", "M"), &[(0, 1_000)])]);
    let report = trim(&conn, &map, true).expect("trim");
    assert!(report.is_empty(), "{report:?}");
    assert_eq!(report.spans_total, 1);
    assert_eq!(report.bytes_total, 10 * ROW_BYTES as i64);
    assert_eq!(
        bounds(&read_spans(&conn, "x", "M", 0, 1_000).expect("read")),
        vec![(100, 199, 10)]
    );
}

/// A market the map does not name loses every span, prints and empty answers alike.
#[test]
fn a_market_off_the_map_goes_whole() {
    let conn = conn();
    file(&conn, ("x", "M"), 100, 199, &ticks(100, 199, 10));
    file(&conn, ("x", "M"), 500, 599, &[]);
    file(&conn, ("x", "N"), 100, 199, &ticks(100, 199, 50));
    let map = keep(&[(("x", "N"), &[(0, 1_000)])]);
    let report = trim(&conn, &map, true).expect("trim");
    assert_eq!(report.spans_dropped, 2);
    assert_eq!(report.spans_cut, 0);
    assert_eq!(report.prints_dropped, 10);
    assert_eq!(report.bytes_dropped, 10 * ROW_BYTES as i64);
    assert!(
        read_spans(&conn, "x", "M", 0, 1_000)
            .expect("read")
            .is_empty()
    );
    assert_eq!(
        bounds(&read_spans(&conn, "x", "N", 0, 1_000).expect("read")),
        vec![(100, 199, 2)]
    );
}

/// A span that straddles the map's edge is cut to the piece inside, prints at the edges
/// included, and keeps its source and its write stamp.
#[test]
fn a_straddling_span_is_cut_to_the_piece_inside() {
    let conn = conn();
    file(&conn, ("x", "M"), 0, 999, &ticks(0, 999, 100));
    let map = keep(&[(("x", "M"), &[(300, 600)])]);
    let report = trim(&conn, &map, true).expect("trim");
    assert_eq!(report.spans_cut, 1);
    assert_eq!(report.spans_dropped, 0);
    // 0..999 by 100 is ten prints; 300, 400, 500 and 600 stay.
    assert_eq!(report.prints_dropped, 6);
    assert_eq!(report.bytes_dropped, 6 * ROW_BYTES as i64);
    let spans = read_spans(&conn, "x", "M", 0, 1_000).expect("read");
    assert_eq!(bounds(&spans), vec![(300, 600, 4)]);
    assert_eq!(spans[0].source, TileSource::Core);
    assert_eq!(
        spans[0]
            .ticks
            .iter()
            .map(|t| t.time_ms as i64)
            .collect::<Vec<_>>(),
        vec![300, 400, 500, 600]
    );
    let updated: i64 = conn
        .query_row("SELECT updated_ms FROM spans", [], |r| r.get(0))
        .expect("stamp");
    assert_eq!(updated, 7, "the piece is not a fresh write");
}

/// A hole in the map inside one span cuts it into two pieces, the hole gone.
#[test]
fn a_hole_in_the_map_splits_a_span() {
    let conn = conn();
    file(&conn, ("x", "M"), 0, 999, &ticks(0, 999, 100));
    let map = keep(&[(("x", "M"), &[(0, 250), (750, 2_000)])]);
    let report = trim(&conn, &map, true).expect("trim");
    assert_eq!((report.spans_cut, report.spans_dropped), (1, 0));
    // 0, 100, 200 and 800, 900 stay; 300..700 go.
    assert_eq!(report.prints_dropped, 5);
    assert_eq!(
        bounds(&read_spans(&conn, "x", "M", 0, 1_000).expect("read")),
        vec![(0, 250, 3), (750, 999, 2)]
    );
}

/// The union of two trades' stretches is one stretch when they overlap: the ground between
/// them is inside it and is never cut — only the outer edges go.
#[test]
fn the_ground_between_overlapping_trades_stays() {
    let conn = conn();
    file(&conn, ("x", "M"), 0, 9_999, &ticks(0, 9_999, 100));
    // Two trades whose stretches overlap at 4_000..5_000.
    let map = keep(&[(("x", "M"), &[(2_000, 5_000), (4_000, 8_000)])]);
    let report = trim(&conn, &map, true).expect("trim");
    assert_eq!(
        bounds(&read_spans(&conn, "x", "M", 0, 10_000).expect("read")),
        vec![(2_000, 8_000, 61)]
    );
    assert_eq!(report.prints_dropped, 100 - 61);
}

/// A dry run counts exactly what an apply would remove and writes nothing.
#[test]
fn a_dry_run_counts_and_leaves_the_file_alone() {
    let conn = conn();
    file(&conn, ("x", "M"), 0, 999, &ticks(0, 999, 100));
    file(&conn, ("y", "M"), 0, 999, &ticks(0, 999, 100));
    let map = keep(&[(("x", "M"), &[(300, 600)])]);
    let dry = trim(&conn, &map, false).expect("dry run");
    assert_eq!(
        bounds(&read_spans(&conn, "x", "M", 0, 1_000).expect("read")),
        vec![(0, 999, 10)]
    );
    assert_eq!(
        bounds(&read_spans(&conn, "y", "M", 0, 1_000).expect("read")),
        vec![(0, 999, 10)]
    );
    let wet = trim(&conn, &map, true).expect("apply");
    assert_eq!(dry, wet);
    assert_eq!(
        (wet.spans_dropped, wet.spans_cut, wet.prints_dropped),
        (1, 1, 16)
    );
}

/// The inventory names every market once and spans the whole file in time.
#[test]
fn the_inventory_lists_markets_and_the_range() {
    let conn = conn();
    assert_eq!(inventory(&conn).expect("empty"), Inventory::default());
    file(&conn, ("x", "M"), 500, 599, &[]);
    file(&conn, ("x", "M"), 100, 199, &ticks(100, 199, 10));
    file(&conn, ("y", "A"), 50, 60, &[]);
    let got = inventory(&conn).expect("inventory");
    assert_eq!(
        got.keys,
        vec![
            ("x".to_string(), "M".to_string()),
            ("y".to_string(), "A".to_string())
        ]
    );
    assert_eq!(got.range_ms, Some((50, 599)));
}

/// The cut keeps a print's bytes as they were, and takes the bounds inclusively.
#[test]
fn cut_blob_keeps_whole_rows_inside_inclusive_bounds() {
    let blob = super::super::pack(ticks(0, 400, 100).iter());
    let cut = cut_blob(&blob, 100, 300);
    assert_eq!(cut.len(), 3 * ROW_BYTES);
    assert_eq!(&cut[..], &blob[ROW_BYTES..4 * ROW_BYTES]);
    assert!(cut_blob(&blob, 401, 500).is_empty());
}
