use super::*;

fn conn() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
    init_schema(&conn).expect("schema");
    conn
}

fn tick(time_ms: i64, side: Side) -> Tick {
    Tick {
        time_ms: time_ms as f64,
        price: 1.5,
        qty: 2.25,
        side,
    }
}

/// The source rides the row and comes back with it.
#[test]
fn a_span_keeps_its_source() {
    let conn = conn();
    insert_span(
        &conn,
        "x",
        "M",
        0,
        99,
        &[tick(50, Side::Buy)],
        TileSource::Core,
        1,
    )
    .expect("core");
    insert_span(&conn, "x", "M", 100, 199, &[], TileSource::Venue, 1).expect("venue");
    let spans = read_spans(&conn, "x", "M", 0, 1_000).expect("read");
    assert_eq!(
        spans.iter().map(|s| s.source).collect::<Vec<_>>(),
        vec![TileSource::Core, TileSource::Venue]
    );
}

fn times(spans: &[StoredSpan]) -> Vec<(i64, i64, Vec<i64>)> {
    spans
        .iter()
        .map(|s| {
            (
                s.from_ms,
                s.to_ms,
                s.ticks.iter().map(|t| t.time_ms as i64).collect(),
            )
        })
        .collect()
}

/// A span round-trips whole: bounds, every print with its stamp, price, size and side.
#[test]
fn a_span_round_trips_with_its_prints() {
    let conn = conn();
    let rows = vec![tick(1_700, Side::Sell), tick(1_100, Side::Buy)];
    insert_span(
        &conn,
        "3:00000000",
        "BTCUSDT",
        1_000,
        1_999,
        &rows,
        TileSource::Venue,
        10,
    )
    .expect("insert");
    let spans = read_spans(&conn, "3:00000000", "BTCUSDT", 0, 5_000).expect("read");
    assert_eq!(times(&spans), vec![(1_000, 1_999, vec![1_100, 1_700])]);
    let first = &spans[0].ticks[0];
    assert_eq!(first.price, 1.5);
    assert_eq!(first.qty, 2.25);
    assert_eq!(first.side, Side::Buy);
    assert_eq!(spans[0].ticks[1].side, Side::Sell);
}

/// A second span overlapping a stored one files only its gaps, never the held stretch again.
#[test]
fn overlapping_insert_files_only_the_gaps() {
    let conn = conn();
    insert_span(
        &conn,
        "x",
        "M",
        1_000,
        1_999,
        &[tick(1_500, Side::Buy)],
        TileSource::Venue,
        10,
    )
    .expect("first");
    insert_span(
        &conn,
        "x",
        "M",
        500,
        2_499,
        &[
            tick(700, Side::Buy),
            tick(1_500, Side::Buy),
            tick(2_300, Side::Buy),
        ],
        TileSource::Venue,
        11,
    )
    .expect("second");
    let spans = read_spans(&conn, "x", "M", 0, 5_000).expect("read");
    assert_eq!(
        times(&spans),
        vec![
            (500, 999, vec![700]),
            (1_000, 1_999, vec![1_500]),
            (2_000, 2_499, vec![2_300]),
        ]
    );
    // Covered whole: nothing more is written.
    insert_span(
        &conn,
        "x",
        "M",
        600,
        2_400,
        &[tick(800, Side::Buy)],
        TileSource::Venue,
        12,
    )
    .expect("third");
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM spans", [], |r| r.get(0))
        .expect("count");
    assert_eq!(count, 3);
}

/// An empty span is stored and read back as an answer, and only spans touching the range come.
#[test]
fn empty_spans_persist_and_reads_are_ranged() {
    let conn = conn();
    insert_span(&conn, "x", "M", 1_000, 1_999, &[], TileSource::Venue, 10).expect("empty");
    insert_span(
        &conn,
        "x",
        "M",
        9_000,
        9_999,
        &[tick(9_500, Side::Buy)],
        TileSource::Venue,
        10,
    )
    .expect("far");
    insert_span(
        &conn,
        "y",
        "M",
        1_000,
        1_999,
        &[tick(1_500, Side::Buy)],
        TileSource::Venue,
        10,
    )
    .expect("other key");
    let spans = read_spans(&conn, "x", "M", 1_500, 2_500).expect("read");
    assert_eq!(times(&spans), vec![(1_000, 1_999, vec![])]);
}

/// A file in another layout is started over rather than read as garbage.
#[test]
fn a_foreign_layout_is_dropped_at_open() {
    let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch(
        "CREATE TABLE spans(exchange TEXT, market TEXT, from_ms INTEGER, to_ms INTEGER,
                            ticks BLOB NOT NULL, updated_ms INTEGER, PRIMARY KEY(exchange, market, from_ms));
         INSERT INTO spans VALUES('x', 'M', 0, 99, X'0102030405060708090a0b0c0d0e0f10', 1);
         PRAGMA user_version = 2;",
    )
    .expect("old layout");
    init_schema(&conn).expect("schema");
    assert!(
        read_spans(&conn, "x", "M", 0, 1_000)
            .expect("read")
            .is_empty()
    );
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("version");
    assert_eq!(version, SCHEMA_VERSION);
    // Same layout: reopening keeps what is there.
    insert_span(
        &conn,
        "x",
        "M",
        0,
        99,
        &[tick(50, Side::Buy)],
        TileSource::Venue,
        1,
    )
    .expect("insert");
    init_schema(&conn).expect("schema again");
    assert_eq!(
        times(&read_spans(&conn, "x", "M", 0, 1_000).expect("read")),
        vec![(0, 99, vec![50])]
    );
}

/// Retention drops what was not touched for two weeks; the byte ceiling drops the oldest first.
#[test]
fn prune_applies_retention_then_the_byte_ceiling() {
    let conn = conn();
    let now = 100 * DAY_MS;
    insert_span(
        &conn,
        "x",
        "M",
        0,
        99,
        &[tick(50, Side::Buy)],
        TileSource::Venue,
        now - 20 * DAY_MS,
    )
    .expect("old");
    insert_span(
        &conn,
        "x",
        "M",
        100,
        199,
        &[tick(150, Side::Buy)],
        TileSource::Venue,
        now - DAY_MS,
    )
    .expect("fresh");
    // A ceiling of this test's own, so nothing here reads a `storage.toml`.
    let ceiling: i64 = 4 * 1024;
    let held = prune(&conn, now, Some(ceiling)).expect("prune");
    assert_eq!(held, ROW_BYTES as i64, "one print left after retention");
    let spans = read_spans(&conn, "x", "M", 0, 1_000).expect("read");
    assert_eq!(times(&spans), vec![(100, 199, vec![150])]);
    // Well past the ceiling: fake it by inserting one huge span older than a small fresh one.
    let huge: Vec<Tick> = (0..(ceiling / ROW_BYTES as i64 + 10))
        .map(|i| tick(1_000 + i, Side::Buy))
        .collect();
    insert_span(
        &conn,
        "x",
        "H",
        1_000,
        1_000_000_000,
        &huge,
        TileSource::Venue,
        now - 2 * DAY_MS,
    )
    .expect("huge");
    let held = prune(&conn, now, Some(ceiling)).expect("prune again");
    assert!(held <= ceiling);
    assert!(
        read_spans(&conn, "x", "H", 0, i64::MAX)
            .expect("read huge")
            .is_empty(),
        "the oldest span paid for the ceiling"
    );
    assert_eq!(
        times(&read_spans(&conn, "x", "M", 0, 1_000).expect("read")),
        vec![(100, 199, vec![150])]
    );
}

/// The packed row is [`ROW_BYTES`] wide and a trailing partial row is ignored rather than read.
#[test]
fn pack_is_fixed_width_and_unpack_ignores_a_torn_tail() {
    // Three months apart: a position held that long packs without an offset to wrap.
    let rows = [
        tick(1_005, Side::Sell),
        tick(1_005 + 90 * DAY_MS, Side::Buy),
    ];
    let mut blob = pack(rows.iter());
    assert_eq!(blob.len(), 2 * ROW_BYTES);
    blob.extend_from_slice(&[1, 2, 3]);
    let back = unpack(&blob);
    assert_eq!(back.len(), 2);
    assert_eq!(back[0].time_ms as i64, 1_005);
    assert_eq!(back[1].time_ms as i64, 1_005 + 90 * DAY_MS);
    assert_eq!(back[0].side, Side::Sell);
    assert_eq!(back[1].side, Side::Buy);
}

/// With no ceiling the byte pass keeps everything retention admits.
#[test]
fn no_ceiling_keeps_everything() {
    let conn = conn();
    let rows: Vec<Tick> = (0..500).map(|i| tick(1_000 + i, Side::Buy)).collect();
    insert_span(&conn, "x", "M", 1_000, 1_999, &rows, TileSource::Venue, 10).expect("insert");
    let held = prune(&conn, 20, None).expect("prune");
    assert_eq!(held, 500 * ROW_BYTES as i64);
    assert_eq!(
        read_spans(&conn, "x", "M", 0, 5_000).expect("read")[0]
            .ticks
            .len(),
        500
    );
}
