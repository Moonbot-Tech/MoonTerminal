use super::codec::encode_legacy;
use super::*;
use crate::feed::types::Side;

/// Milliseconds in a day, for stamps the tests spread a year apart.
const DAY_MS: i64 = 86_400_000;

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

/// A row as an older build filed it: the legacy table, fixed-width rows.
fn file_legacy(
    conn: &rusqlite::Connection,
    market: &str,
    from_ms: i64,
    to_ms: i64,
    ticks: &[Tick],
    updated_ms: i64,
) {
    conn.execute(
        "INSERT INTO spans(exchange, market, from_ms, to_ms, ticks, source, updated_ms)
         VALUES('x', ?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            market,
            from_ms,
            to_ms,
            encode_legacy(ticks),
            TileSource::Core.code(),
            updated_ms
        ],
    )
    .expect("legacy row");
}

fn rows_in(conn: &rusqlite::Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .expect("count")
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
    assert_eq!(rows_in(&conn, "packs"), 3);
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

/// The offset-walk repair drops Gate futures VENUE spans once: the core's own spans of the same
/// market stay, other venues' spans stay, and a second open drops nothing more.
#[test]
fn the_gate_futures_repair_drops_only_venue_spans_of_that_platform_once() {
    let conn = conn();
    // 9 = Gate futures, 8 = Gate spot, 4 = Binance futures (`venue::venue`).
    // Distinct stretches per row: the store files only what is not yet held.
    for (key, source, from_ms) in [
        ("9:00000000", TileSource::Venue, 0),
        ("9:00000000", TileSource::Core, 100),
        ("8:00000000", TileSource::Venue, 0),
        ("4:00000000", TileSource::Venue, 0),
    ] {
        insert_span(
            &conn,
            key,
            "M",
            from_ms,
            from_ms + 99,
            &[tick(from_ms + 50, Side::Buy)],
            source,
            1,
        )
        .expect("insert");
    }
    conn.execute("DELETE FROM repairs", [])
        .expect("forget the repair");
    repair_gate_futures_offset_walks(&conn).expect("repair");
    let sources = |key: &str| -> Vec<i64> {
        conn.prepare("SELECT source FROM packs WHERE exchange = ?1 ORDER BY source")
            .expect("prepare")
            .query_map([key], |r| r.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows")
    };
    assert_eq!(
        sources("9:00000000"),
        vec![1],
        "the venue span went, the core's stayed"
    );
    assert_eq!(sources("8:00000000"), vec![0]);
    assert_eq!(sources("4:00000000"), vec![0]);
    // Recorded: a venue span filed after the repair survives the next open.
    insert_span(
        &conn,
        "9:00000000",
        "M",
        200,
        299,
        &[tick(250, Side::Buy)],
        TileSource::Venue,
        2,
    )
    .expect("insert");
    init_schema(&conn).expect("schema again");
    assert_eq!(sources("9:00000000"), vec![0, 1]);
    assert!(
        !exchange_key_is_gate_futures("x:1"),
        "an unknown ordinal is left alone"
    );
}

/// Age alone drops nothing — a span untouched for a year is kept whole — and the byte ceiling
/// drops the spans written longest ago first.
#[test]
fn prune_applies_only_the_byte_ceiling_never_an_age_limit() {
    let conn = conn();
    let now = 400 * DAY_MS;
    insert_span(
        &conn,
        "x",
        "M",
        0,
        99,
        &[tick(50, Side::Buy)],
        TileSource::Venue,
        now - 365 * DAY_MS,
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
    let held = prune(&conn, Some(ceiling)).expect("prune");
    assert_eq!(
        held,
        held_bytes(&conn).expect("bytes"),
        "both prints kept: age is not a rule"
    );
    assert!(held < ceiling);
    let spans = read_spans(&conn, "x", "M", 0, 1_000).expect("read");
    assert_eq!(
        times(&spans),
        vec![(0, 99, vec![50]), (100, 199, vec![150])]
    );
    // Well past the ceiling: fake it by filing one huge span written more recently than the
    // year-old one — the year-old span goes first, then the huge one, and the fresh one stays.
    // Filed in the legacy layout, which does not compress, so the ceiling is crossed for sure
    // and the eviction is seen to pick across both tables.
    let huge: Vec<Tick> = (0..(ceiling / codec::LEGACY_ROW_BYTES as i64 + 10))
        .map(|i| tick(1_000 + i, Side::Buy))
        .collect();
    file_legacy(&conn, "H", 1_000, 1_000_000_000, &huge, now - 2 * DAY_MS);
    let held = prune(&conn, Some(ceiling)).expect("prune again");
    assert!(held <= ceiling);
    assert!(
        read_spans(&conn, "x", "H", 0, i64::MAX)
            .expect("read huge")
            .is_empty(),
        "the spans written longest ago paid for the ceiling"
    );
    assert_eq!(
        times(&read_spans(&conn, "x", "M", 0, 1_000).expect("read")),
        vec![(100, 199, vec![150])]
    );
}

/// With no ceiling everything is kept, however old: a span last touched at the epoch survives
/// an open a year later.
#[test]
fn no_ceiling_keeps_everything() {
    let conn = conn();
    let rows: Vec<Tick> = (0..500).map(|i| tick(1_000 + i, Side::Buy)).collect();
    insert_span(&conn, "x", "M", 1_000, 1_999, &rows, TileSource::Venue, 10).expect("insert");
    let held = prune(&conn, None).expect("prune");
    assert_eq!(held, held_bytes(&conn).expect("bytes"));
    assert_eq!(
        read_spans(&conn, "x", "M", 0, 5_000).expect("read")[0]
            .ticks
            .len(),
        500
    );
}

/// The bounds read answers what is held over a stretch — every span that touches it, in order,
/// the empty ones included — without the prints.
#[test]
fn span_bounds_answer_what_is_held_without_the_prints() {
    let conn = conn();
    for (from, to) in [(3_000, 3_999), (1_000, 1_999), (9_000, 9_999)] {
        insert_span(&conn, "x", "M", from, to, &[], TileSource::Venue, 1).expect("insert");
    }
    insert_span(&conn, "x", "OTHER", 1_000, 1_999, &[], TileSource::Venue, 1).expect("insert");
    let held = read_span_bounds(&conn, "x", "M", 1_500, 3_500).expect("read");
    assert_eq!(held, vec![(1_000, 1_999), (3_000, 3_999)]);
    assert!(
        read_span_bounds(&conn, "x", "M", 5_000, 6_000)
            .expect("read")
            .is_empty()
    );
}

/// A file an older build wrote is served as it is: legacy rows and packed rows come back
/// together, in order, and a new answer files only around what the legacy rows hold.
#[test]
fn legacy_rows_are_read_and_respected_before_they_are_moved() {
    let conn = conn();
    file_legacy(&conn, "M", 1_000, 1_999, &[tick(1_500, Side::Sell)], 5);
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
    .expect("insert");
    let spans = read_spans(&conn, "x", "M", 0, 5_000).expect("read");
    assert_eq!(
        times(&spans),
        vec![
            (500, 999, vec![700]),
            (1_000, 1_999, vec![1_500]),
            (2_000, 2_499, vec![2_300]),
        ]
    );
    assert_eq!(
        spans[1].source,
        TileSource::Core,
        "the legacy row, untouched"
    );
    assert_eq!(spans[1].ticks[0].side, Side::Sell);
    assert_eq!(
        read_span_bounds(&conn, "x", "M", 0, 5_000).expect("bounds"),
        vec![(500, 999), (1_000, 1_999), (2_000, 2_499)]
    );
}

/// The move takes every legacy row into the packed table in batches, each print, bound, source
/// and write stamp intact, and leaves nothing behind or doubled.
#[test]
fn a_repack_moves_every_legacy_row_intact() {
    let conn = conn();
    for i in 0..6i64 {
        let from = i * 10_000;
        let prints: Vec<Tick> = (0..300)
            .map(|k| Tick {
                time_ms: (from + k * 7) as f64,
                price: 10.0 + k as f32 * 0.25,
                qty: 0.5 + k as f32,
                side: if k % 3 == 0 { Side::Sell } else { Side::Buy },
            })
            .collect();
        file_legacy(&conn, "M", from, from + 9_999, &prints, 100 + i);
    }
    // An empty answer is a span too.
    file_legacy(&conn, "M", 90_000, 90_999, &[], 1);
    let before = read_spans(&conn, "x", "M", 0, i64::MAX).expect("before");
    let legacy_bytes = held_bytes(&conn).expect("bytes");
    let mut held = legacy_bytes;
    // A budget of about one row: several batches, the last one reporting nothing left.
    let mut batches = 0;
    loop {
        let batch = repack_batch(&conn, 300 * codec::LEGACY_ROW_BYTES as i64).expect("batch");
        held += batch.packed_bytes - batch.legacy_bytes;
        batches += 1;
        if !batch.more {
            break;
        }
    }
    assert!(batches > 1, "the budget split the move");
    assert_eq!(rows_in(&conn, "spans"), 0);
    assert_eq!(rows_in(&conn, "packs"), 7);
    assert_eq!(
        held,
        held_bytes(&conn).expect("bytes"),
        "the carried count is the file's"
    );
    assert!(
        held * 2 < legacy_bytes,
        "{held} packed against {legacy_bytes} legacy"
    );
    let after = read_spans(&conn, "x", "M", 0, i64::MAX).expect("after");
    assert_eq!(times(&after), times(&before));
    for (a, b) in after.iter().zip(&before) {
        assert_eq!(a.source, b.source);
        for (x, y) in a.ticks.iter().zip(&b.ticks) {
            assert_eq!(
                (x.price.to_bits(), x.qty.to_bits(), x.side),
                (y.price.to_bits(), y.qty.to_bits(), y.side)
            );
        }
    }
    let stamps: Vec<i64> = conn
        .prepare("SELECT updated_ms FROM packs ORDER BY from_ms")
        .expect("prepare")
        .query_map([], |r| r.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    assert_eq!(stamps, vec![100, 101, 102, 103, 104, 105, 1]);
    assert!(!legacy_rows_left(&conn).expect("left"));
}

/// The worker's side of the move: it steps until the legacy table is empty, then compacts and
/// stops, the carried byte count matching the file.
#[test]
fn the_repack_tally_steps_to_the_end_and_compacts() {
    let conn = conn();
    for i in 0..3i64 {
        file_legacy(
            &conn,
            "M",
            i * 1_000,
            i * 1_000 + 999,
            &[tick(i * 1_000 + 5, Side::Buy)],
            i,
        );
    }
    let mut held = held_bytes(&conn).expect("bytes");
    let mut tally = RepackTally::start();
    let mut steps = 0;
    while tally.step(&conn, &mut held) {
        steps += 1;
        assert!(steps < 100, "the move ends");
    }
    assert_eq!(tally.spans, 3);
    assert_eq!(rows_in(&conn, "spans"), 0);
    assert_eq!(held, held_bytes(&conn).expect("bytes"));
}

/// A packed row that does not decode is dropped on read — never served as a quiet span, never
/// left to claim its stretch as held.
#[test]
fn a_packed_row_that_does_not_decode_is_dropped_on_read() {
    let conn = conn();
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
    .expect("good");
    conn.execute(
        "INSERT INTO packs(exchange, market, from_ms, to_ms, ticks, prints, source, updated_ms)
         VALUES('x', 'M', 100, 199, X'05ffffff', 5, 0, 1)",
        [],
    )
    .expect("damaged row");
    let mut dropped = 0;
    let spans = read_spans_dropping(&conn, "x", "M", 0, 1_000, &mut dropped).expect("read");
    assert_eq!(times(&spans), vec![(0, 99, vec![50])]);
    assert_eq!(
        dropped, 4,
        "the deleted blob's bytes, for the worker's count"
    );
    assert_eq!(
        read_span_bounds(&conn, "x", "M", 0, 1_000).expect("bounds"),
        vec![(0, 99)],
        "its stretch is no longer held"
    );
}

/// The ceiling picks the row written longest ago whichever table it sits in.
#[test]
fn the_ceiling_evicts_across_both_tables_by_write_age() {
    let conn = conn();
    insert_span(
        &conn,
        "x",
        "M",
        0,
        99,
        &[tick(50, Side::Buy)],
        TileSource::Venue,
        30,
    )
    .expect("packed, newest");
    file_legacy(&conn, "M", 100, 199, &[tick(150, Side::Buy)], 10);
    insert_span(
        &conn,
        "x",
        "M",
        200,
        299,
        &[tick(250, Side::Buy)],
        TileSource::Venue,
        20,
    )
    .expect("packed, middle");
    let held = held_bytes(&conn).expect("bytes");
    let legacy = codec::LEGACY_ROW_BYTES as i64;
    let after = trim_to_ceiling(&conn, held, Some(held - legacy)).expect("trim");
    assert_eq!(after, held - legacy);
    assert_eq!(
        read_span_bounds(&conn, "x", "M", 0, 1_000).expect("bounds"),
        vec![(0, 99), (200, 299)],
        "the legacy row was the oldest"
    );
    let after = trim_to_ceiling(&conn, after, Some(after - 1)).expect("trim again");
    assert!(after < held - legacy);
    assert_eq!(
        read_span_bounds(&conn, "x", "M", 0, 1_000).expect("bounds"),
        vec![(0, 99)],
        "then the older packed row"
    );
}

/// Measurement on a real file, not a test: what the move from `spans` to `packs` costs and what
/// it buys — file size, read latency before and after, the longest batch a queued op could wait
/// behind, the compaction, an insert, and a cleanup that cuts every span.
///
/// Destructive: it rewrites the file it is given. Point it at a copy:
///
/// ```text
/// MOON_TRADES_BENCH_DB=<copy of trades.sqlite> cargo test -p moon-core --lib \
///     measure_the_repack_on_a_real_file -- --ignored --nocapture
/// ```
#[test]
#[ignore = "measurement: needs MOON_TRADES_BENCH_DB, a copy of a real trades.sqlite"]
fn measure_the_repack_on_a_real_file() {
    use std::time::Instant;

    let Ok(path) = std::env::var("MOON_TRADES_BENCH_DB") else {
        eprintln!("MOON_TRADES_BENCH_DB not set; nothing measured");
        return;
    };
    let path = std::path::PathBuf::from(path);
    let file_mb =
        |p: &std::path::Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0) as f64 / 1e6;
    let wal_path = std::path::PathBuf::from(format!("{}-wal", path.display()));
    let conn = rusqlite::Connection::open(&path).expect("open");
    let t = Instant::now();
    init_schema(&conn).expect("schema");
    println!("open + schema: {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).expect(sql) };
    let legacy_prints = count("SELECT COALESCE(SUM(LENGTH(ticks) / 20), 0) FROM spans");
    println!(
        "before: file {:.1} MB, wal {:.1} MB, {} legacy span(s), {} packed, {} print(s), {:.1} MB of blobs",
        file_mb(&path),
        file_mb(&wal_path),
        count("SELECT COUNT(*) FROM spans"),
        count("SELECT COUNT(*) FROM packs"),
        legacy_prints,
        held_bytes(&conn).expect("bytes") as f64 / 1e6
    );

    // A fixed sample of spans to read before and after: every 25th row, big and small alike.
    let sample: Vec<(String, String, i64, i64)> = conn
        .prepare("SELECT exchange, market, from_ms, to_ms FROM spans WHERE rowid % 25 = 0")
        .expect("prepare")
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    let read_all = |label: &str| -> (u64, Vec<usize>) {
        let mut times_us: Vec<u128> = Vec::with_capacity(sample.len());
        let mut prints = 0u64;
        let mut per_span = Vec::with_capacity(sample.len());
        for (exchange, market, from_ms, to_ms) in &sample {
            let t = Instant::now();
            let spans = read_spans(&conn, exchange, market, *from_ms, *to_ms).expect("read");
            times_us.push(t.elapsed().as_micros());
            let n: usize = spans.iter().map(|s| s.ticks.len()).sum();
            prints += n as u64;
            per_span.push(n);
        }
        times_us.sort_unstable();
        let pct = |p: f64| times_us[((times_us.len() - 1) as f64 * p) as usize];
        println!(
            "read {label}: {} window(s), {prints} print(s), p50 {} us, p90 {} us, p99 {} us, max {} us, total {:.0} ms",
            sample.len(),
            pct(0.5),
            pct(0.9),
            pct(0.99),
            times_us.last().copied().unwrap_or(0),
            times_us.iter().sum::<u128>() as f64 / 1e3
        );
        (prints, per_span)
    };
    let bounds_all = |label: &str| {
        let t = Instant::now();
        for (exchange, market, from_ms, to_ms) in &sample {
            read_span_bounds(&conn, exchange, market, *from_ms, *to_ms).expect("bounds");
        }
        println!(
            "bounds {label}: {} window(s) in {:.1} ms",
            sample.len(),
            t.elapsed().as_secs_f64() * 1e3
        );
    };
    let (before_prints, before_each) = read_all("legacy");
    bounds_all("legacy");

    let mut held = held_bytes(&conn).expect("bytes");
    let mut tally = RepackTally::start();
    while tally.step(&conn, &mut held) {}
    println!(
        "repack: {} span(s) in {} batch(es), busy {:.2} s, longest batch {} ms, wall {:.2} s (compaction included)",
        tally.spans,
        tally.batches,
        tally.busy.as_secs_f64(),
        tally.longest.as_millis(),
        tally.started.elapsed().as_secs_f64()
    );
    let packed_prints = count("SELECT COALESCE(SUM(prints), 0) FROM packs");
    println!(
        "after: file {:.1} MB, wal {:.1} MB, {} legacy span(s), {} packed, {} print(s), {:.1} MB of blobs ({:.2} bytes a print)",
        file_mb(&path),
        file_mb(&wal_path),
        count("SELECT COUNT(*) FROM spans"),
        count("SELECT COUNT(*) FROM packs"),
        packed_prints,
        held as f64 / 1e6,
        held as f64 / packed_prints.max(1) as f64
    );
    assert_eq!(packed_prints, legacy_prints, "every print moved");
    let (after_prints, after_each) = read_all("packed");
    bounds_all("packed");
    assert_eq!(after_prints, before_prints);
    assert_eq!(
        after_each, before_each,
        "every window reads the same prints"
    );

    // An insert of a big harvest under a key of its own: the encode is the whole cost.
    let (exchange, market, from_ms, to_ms) = sample
        .iter()
        .zip(&before_each)
        .max_by_key(|(_, n)| **n)
        .map(|(s, _)| s.clone())
        .expect("a sample");
    let big = read_spans(&conn, &exchange, &market, from_ms, to_ms).expect("read");
    let ticks: Vec<Tick> = big.iter().flat_map(|s| s.ticks.iter().copied()).collect();
    let t = Instant::now();
    let tx = conn.unchecked_transaction().expect("tx");
    let wrote = insert_span(
        &tx,
        &exchange,
        "BENCH",
        from_ms,
        to_ms,
        &ticks,
        TileSource::Venue,
        1,
    )
    .expect("insert");
    tx.commit().expect("commit");
    println!(
        "insert: {} print(s) → {} bytes in {:.1} ms",
        ticks.len(),
        wrote,
        t.elapsed().as_secs_f64() * 1e3
    );

    // A cleanup that cuts every span in two: every blob decoded and its pieces re-encoded.
    let inv = trim::inventory(&conn).expect("inventory");
    let mut keep = KeepMap::new();
    for (exchange, market) in &inv.keys {
        let spans: Vec<(i64, i64)> = read_span_bounds(&conn, exchange, market, i64::MIN, i64::MAX)
            .expect("bounds")
            .into_iter()
            .map(|(from, to)| (from, from + (to - from) / 2))
            .collect();
        keep.insert(
            (exchange.clone(), market.clone()),
            super::super::coverage::Coverage::from_spans(spans),
        );
    }
    let t = Instant::now();
    let report = trim::trim(&conn, &keep, false).expect("dry run");
    println!(
        "cleanup dry run cutting every span: {:.2} s, {} cut, {} print(s) / {:.1} MB would go",
        t.elapsed().as_secs_f64(),
        report.spans_cut,
        report.prints_dropped,
        report.bytes_dropped as f64 / 1e6
    );
    let t = Instant::now();
    let report = trim::trim(&conn, &keep, true).expect("apply");
    let applied = t.elapsed();
    let t = Instant::now();
    crate::db::wal::vacuum(&conn).expect("vacuum");
    println!(
        "cleanup applied: {:.2} s, vacuum {:.2} s, {} cut; file {:.1} MB, wal {:.1} MB",
        applied.as_secs_f64(),
        t.elapsed().as_secs_f64(),
        report.spans_cut,
        file_mb(&path),
        file_mb(&wal_path)
    );
}

/// Measurement on a real file through the real worker: open it the way the terminal does and
/// read windows back-to-back while the worker moves `spans` into `packs` between them — how long
/// a read waits, and how many give up at [`READ_TIMEOUT`] and would cost a refetch.
///
/// Destructive like [`measure_the_repack_on_a_real_file`]; point it at a copy that still holds
/// legacy rows:
///
/// ```text
/// MOON_TRADES_BENCH_DB=<copy of trades.sqlite> cargo test -p moon-core --lib \
///     measure_reads_while_the_worker_repacks -- --ignored --nocapture
/// ```
#[test]
#[ignore = "measurement: needs MOON_TRADES_BENCH_DB, a copy of a real trades.sqlite"]
fn measure_reads_while_the_worker_repacks() {
    use std::time::Instant;

    let Ok(path) = std::env::var("MOON_TRADES_BENCH_DB") else {
        eprintln!("MOON_TRADES_BENCH_DB not set; nothing measured");
        return;
    };
    let path = std::path::PathBuf::from(path);
    let sample: Vec<(String, String, i64, i64)> = {
        let conn = rusqlite::Connection::open(&path).expect("open");
        let sample = conn
            .prepare("SELECT exchange, market, from_ms, to_ms FROM spans WHERE rowid % 25 = 0")
            .expect("prepare")
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        sample
    };
    assert!(!sample.is_empty(), "the copy holds no legacy rows to move");
    let legacy_left = |conn: &rusqlite::Connection| -> i64 {
        conn.query_row("SELECT COUNT(*) FROM spans", [], |r| r.get(0))
            .unwrap_or(-1)
    };
    let watcher = rusqlite::Connection::open(&path).expect("watcher");
    let cache = TradeCache::open(path.clone()).expect("worker");
    let started = Instant::now();
    let mut waits_us: Vec<u128> = Vec::new();
    let mut timeouts = 0u32;
    let mut prints = 0u64;
    let mut done_at = None;
    let mut i = 0usize;
    while started.elapsed() < Duration::from_secs(60) {
        let (exchange, market, from_ms, to_ms) = &sample[i % sample.len()];
        i += 1;
        let t = Instant::now();
        match cache.read(exchange, market, *from_ms, *to_ms) {
            Some(spans) => {
                waits_us.push(t.elapsed().as_micros());
                prints += spans.iter().map(|s| s.ticks.len() as u64).sum::<u64>();
            }
            None => timeouts += 1,
        }
        if done_at.is_none() && i.is_multiple_of(20) && legacy_left(&watcher) == 0 {
            done_at = Some(started.elapsed());
        }
        // Keep reading a while past the end: the compaction runs after the last batch.
        if done_at.is_some_and(|done| started.elapsed() > done + Duration::from_secs(2)) {
            break;
        }
    }
    waits_us.sort_unstable();
    let pct = |p: f64| waits_us[((waits_us.len() - 1) as f64 * p) as usize];
    println!(
        "live worker: {} read(s) answered, {timeouts} timed out at {} ms, {prints} print(s); \
         wait p50 {} us, p90 {} us, p99 {} us, max {} us; legacy table empty after {:?}",
        waits_us.len(),
        READ_TIMEOUT.as_millis(),
        pct(0.5),
        pct(0.9),
        pct(0.99),
        waits_us.last().copied().unwrap_or(0),
        done_at
    );
    assert_eq!(legacy_left(&watcher), 0, "the worker moved everything");
}
