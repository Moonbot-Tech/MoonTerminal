use super::*;
use crate::market::candles::estimate_quote_volume;

fn candle(t: f64, p: f32) -> ChartCandle {
    ChartCandle {
        t_open_ms: t,
        open: p,
        high: p + 1.0,
        low: p - 1.0,
        close: p + 0.5,
        volume: 10.0,
        quote_volume: 0.0,
    }
}

/// Encodes cache rows independently of the cache writer so migration tests can preserve real v1
/// bytes while asserting the reader's public row values.
fn packed_rows(rows: &[ChartCandle], day_start: i64, include_quote: bool) -> Vec<u8> {
    let mut blob = Vec::new();
    for row in rows {
        blob.extend_from_slice(&((row.t_open_ms as i64 - day_start) as u32).to_le_bytes());
        for value in [row.open, row.high, row.low, row.close, row.volume] {
            blob.extend_from_slice(&value.to_le_bytes());
        }
        if include_quote {
            blob.extend_from_slice(&row.quote_volume.to_le_bytes());
        }
    }
    blob
}

/// `market/kline_cache.rs:read_rows` dropping the v1 table read or letting it beat v2 makes
/// pre-upgrade candles disappear or replaces current quote turnover with a legacy estimate.
#[test]
fn cache_reads_both_formats_and_v2_wins_per_timestamp() {
    let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
    init_schema(&conn).expect("v2 schema");
    let day = 20_000 * DAY_MS;
    let mut legacy_only = candle(day as f64, 10.0);
    legacy_only.quote_volume = 0.0;
    let mut legacy_shared = candle((day + 60_000) as f64, 20.0);
    legacy_shared.quote_volume = 0.0;
    let mut current_shared = candle((day + 60_000) as f64, 90.0);
    current_shared.quote_volume = 900.0;
    let mut current_only = candle((day + 120_000) as f64, 30.0);
    current_only.quote_volume = 300.0;
    conn.execute(
        "INSERT INTO chunks(exchange, market, kind, day, rows, updated_ms) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["x", "PAIR", 5, day / DAY_MS, packed_rows(&[legacy_only, legacy_shared], day, false), 0],
    ).expect("legacy rows");
    conn.execute(
        "INSERT INTO chunks_v2(exchange, market, kind, day, rows, updated_ms) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["x", "PAIR", 5, day / DAY_MS, packed_rows(&[current_shared, current_only], day, true), 0],
    ).expect("current rows");

    let rows = read_rows(&conn, "x", "PAIR", 5, day, day + 120_000).expect("merged read");
    assert_eq!(
        rows.iter()
            .map(|row| row.t_open_ms as i64)
            .collect::<Vec<_>>(),
        vec![day, day + 60_000, day + 120_000]
    );
    assert_eq!(
        rows[1].open, 90.0,
        "the precise v2 row wins over the same legacy timestamp"
    );
    assert_eq!(rows[1].quote_volume, 900.0);
    assert_eq!(
        rows[0].quote_volume,
        estimate_quote_volume(
            legacy_only.volume,
            legacy_only.open,
            legacy_only.high,
            legacy_only.low,
            legacy_only.close
        )
    );
}

/// `market/kline_cache.rs:upsert_one` writing into `chunks` would replace the 24-byte legacy blob
/// with a 28-byte row, corrupting it for an older executable and any unupgraded reader.
#[test]
fn cache_merges_only_into_v2_without_rewriting_legacy_chunk_bytes() {
    let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
    init_schema(&conn).expect("v2 schema");
    let day = 20_001 * DAY_MS;
    let legacy = candle(day as f64, 10.0);
    let original = packed_rows(&[legacy], day, false);
    conn.execute(
        "INSERT INTO chunks(exchange, market, kind, day, rows, updated_ms) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["x", "PAIR", 5, day / DAY_MS, &original, 0],
    ).expect("legacy row");
    let mut incoming = candle((day + 60_000) as f64, 20.0);
    incoming.quote_volume = 200.0;
    assert!(upsert_one(&conn, "x", "PAIR", 5, &[incoming], 1).expect("v2 merge"));
    let after: Vec<u8> = conn
        .query_row(
            "SELECT rows FROM chunks WHERE exchange='x' AND market='PAIR' AND kind=5 AND day=?1",
            [day / DAY_MS],
            |row| row.get(0),
        )
        .expect("legacy chunk remains");
    assert_eq!(
        after, original,
        "the immutable v1 blob must remain byte-identical"
    );
}

/// `market/kline_cache.rs:init_schema` retaining only `chunks` lets `chunks_v2` grow forever,
/// eventually consuming the cache disk budget even though the same market data has aged out.
#[test]
fn retention_removes_expired_rows_from_both_cache_tables() {
    let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
    init_schema(&conn).expect("v2 schema");
    let kind = 5;
    let today = now_unix_ms() / DAY_MS;
    let expired = today - retention_days(kind) - 1;
    let current = today;
    for (table, quote) in [("chunks", false), ("chunks_v2", true)] {
        for day in [expired, current] {
            let mut row = candle((day * DAY_MS) as f64, 10.0);
            row.quote_volume = 100.0;
            let sql = format!(
                "INSERT INTO {table}(exchange, market, kind, day, rows, updated_ms) VALUES(?1, ?2, ?3, ?4, ?5, ?6)"
            );
            conn.execute(
                &sql,
                rusqlite::params![
                    "x",
                    "PAIR",
                    kind,
                    day,
                    packed_rows(&[row], day * DAY_MS, quote),
                    0
                ],
            )
            .expect("seed retention row");
        }
    }
    init_schema(&conn).expect("retention rerun");
    for table in ["chunks", "chunks_v2"] {
        let expired_count: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE day=?1"),
                [expired],
                |row| row.get(0),
            )
            .expect("expired count");
        let current_count: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE day=?1"),
                [current],
                |row| row.get(0),
            )
            .expect("current count");
        assert_eq!(expired_count, 0, "{table} drops expired chunks");
        assert_eq!(current_count, 1, "{table} keeps current chunks");
    }
}

#[test]
fn pack_unpack_roundtrip() {
    let day_start = 19_000i64 * DAY_MS;
    let rows = vec![
        candle(day_start as f64, 5.0),
        candle((day_start + 60_000) as f64, 6.0),
    ];
    let blob = pack_rows_v2(rows.iter(), day_start);
    assert_eq!(blob.len(), 2 * ROW_BYTES_V2);
    let back = unpack_rows_v2(&blob, day_start);
    assert_eq!(back.len(), 2);
    assert_eq!(back[0].t_open_ms, rows[0].t_open_ms);
    assert_eq!(back[1].open, 6.0);
    assert_eq!(back[1].close, 6.5);
}

#[test]
fn merge_dedups_and_read_filters() {
    let dir = std::env::temp_dir().join(format!("kline-cache-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("klines-test.sqlite");
    let _ = std::fs::remove_file(&path);
    let cache = KlineCache::open(path.clone()).expect("open cache");
    let day = (now_unix_ms() / DAY_MS) * DAY_MS;
    // Two overlapping writes: the second one updates the candle at t=day.
    cache.merge(
        "7:0".into(),
        "BTCUSDT".into(),
        1,
        vec![candle(day as f64, 5.0), candle((day + 60_000) as f64, 6.0)],
    );
    cache.merge(
        "7:0".into(),
        "BTCUSDT".into(),
        1,
        vec![candle(day as f64, 9.0)],
    );
    // Let the worker drain its queue; reads use the same channel, preserving order.
    let rows = cache
        .read_range("7:0", "BTCUSDT", 1, day, day + DAY_MS)
        .expect("чтение не должно упасть по таймауту");
    assert_eq!(rows.len(), 2, "дедуп по t_open внутри дня");
    assert_eq!(rows[0].open, 9.0, "поздняя заливка авторитетнее");
    // Rows from another kind or market are not visible.
    assert!(
        cache
            .read_range("7:0", "BTCUSDT", 5, day, day + DAY_MS)
            .is_some_and(|r| r.is_empty())
    );
    assert!(
        cache
            .read_range("7:0", "ETHUSDT", 1, day, day + DAY_MS)
            .is_some_and(|r| r.is_empty())
    );
    let _ = std::fs::remove_file(&path);
}

/// `upsert_one` returning bare `Ok(())` is what let the liveness line fire for a cycle that stored
/// nothing: its filter drops non-finite or non-positive timestamps and non-positive `high`, and an
/// input where every row fails is a successful call with no write behind it. The caller can only
/// tell those apart if this reports it.
#[test]
fn a_merge_of_only_invalid_rows_reports_that_it_wrote_nothing() {
    let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
    init_schema(&conn).expect("schema");
    let day = (now_unix_ms() / DAY_MS) * DAY_MS;
    let now = now_unix_ms();

    let mut rejected = candle(day as f64, 5.0);
    rejected.high = 0.0;
    let wrote = upsert_one(&conn, "7:0", "BTCUSDT", 1, &[rejected], now).expect("filtered merge");
    assert!(!wrote, "every row was filtered, so nothing was stored");

    let wrote = upsert_one(&conn, "7:0", "BTCUSDT", 1, &[candle(day as f64, 5.0)], now)
        .expect("valid merge");
    assert!(wrote, "a row that passes the filter is written");
}

/// The liveness line is a heartbeat, and a heartbeat repeated per key is not one: the per-key form
/// reset its deduplication set on every launch and wrote ~4700 lines within minutes of each start,
/// 32803 a day. One line per writer, whatever it goes on to merge.
///
/// Asserted on the RETURN value rather than the latch, because the latch after the second call is
/// indistinguishable from the latch after the first — a test written that way passes even with the
/// one-shot guard deleted.
#[test]
fn the_cache_announces_itself_once_per_writer() {
    let mut announced = false;

    assert!(
        log_active_once(&mut announced),
        "the first committed merge announces the cache"
    );
    assert!(
        !log_active_once(&mut announced),
        "later cycles must not re-announce"
    );
    assert!(
        !log_active_once(&mut announced),
        "and the latch does not come back"
    );
}

/// The set must stay EMPTY while the trace is off: `HashSet::insert` takes its key by value, so
/// populating it unconditionally allocated two `String`s per merged item — ~5500 markets a cycle —
/// to decide whether to write a line nobody was reading.
///
/// Both branches are exercised here because the level check is a parameter; asserting against the
/// process-global logger instead would pass for a function that simply returned `false`, and would
/// flake the moment another test in this 989-test binary installed a logger.
#[test]
fn the_trace_set_only_fills_while_the_trace_is_on() {
    let mut seen = std::collections::HashSet::new();

    assert!(
        !trace_first_merge(&mut seen, ("2:00000000", "BTCUSDT", 1), false),
        "a disabled trace reports nothing to log"
    );
    assert!(seen.is_empty(), "and allocates nothing to remember it by");

    assert!(
        trace_first_merge(&mut seen, ("2:00000000", "BTCUSDT", 1), true),
        "the first merge of a key is worth a line"
    );
    assert!(
        !trace_first_merge(&mut seen, ("2:00000000", "BTCUSDT", 1), true),
        "the second is not — a cycle spans thousands of markets"
    );
    assert!(
        trace_first_merge(&mut seen, ("2:00000000", "BTCUSDT", 5), true),
        "another kind of the same market is a different key"
    );
}

/// The per-key detail runs once per merged item on the cache write path — thousands of markets —
/// so the default filter has to drop it. It admits `moon_core=info`, hence anything at or below
/// Info reaches the Log panel's 5000-record ring.
#[test]
fn the_merge_trace_stays_below_the_default_filter() {
    assert!(
        MERGE_TRACE_LEVEL > log::Level::Info,
        "the default filter admits Info and above; {MERGE_TRACE_LEVEL} would reach the Log panel"
    );
}

/// The helpers are worth nothing unless both merge arms go through them, and a test that exercises
/// them in isolation stays green while the call sites are deleted.
///
/// Positions rather than a fixed window: slicing a byte window out of a file that holds Cyrillic
/// can land mid-character and panic, and a window wide enough to be robust eventually reaches an
/// unrelated macro. Comparing the offsets of the nearest preceding items says the same thing
/// exactly, and `find`/`rfind` always return char boundaries.
#[test]
fn both_merge_arms_trace_and_announce_through_the_helpers() {
    let source = include_str!("../kline_cache.rs");

    for message in [
        "kline cache: first rows {exchange}/{market}/kind{kind_min}: {}",
        "kline cache: first rows {}/{}/kind{}: {}",
    ] {
        let at = source
            .find(message)
            .unwrap_or_else(|| panic!("the merge trace `{message}` is gone"));
        let head = &source[..at];
        let emit = head
            .rfind("log::log!")
            .unwrap_or_else(|| panic!("`{message}` must be emitted through log::log!"));
        for fixed in ["log::info!", "log::warn!", "log::error!"] {
            if let Some(other) = head.rfind(fixed) {
                assert!(
                    other < emit,
                    "`{message}` is emitted with {fixed}, which the default filter admits"
                );
            }
        }
        let level = head
            .rfind("MERGE_TRACE_LEVEL")
            .expect("the level constant must reach this emit");
        assert!(
            level > emit,
            "`{message}` must take its level from the constant, not a literal"
        );
    }

    // Definition plus both call sites, for each helper.
    assert_eq!(
        source.matches("trace_first_merge(").count(),
        3,
        "both arms must bound the trace to the first merge per key"
    );
    assert_eq!(
        source.matches("log_active_once(").count(),
        3,
        "both arms must announce through the one-shot helper"
    );

    // The batch arm announces inside the commit's Ok arm, the single arm inside `if wrote`.
    let batch_commit = source
        .find("match tx.commit()")
        .expect("the batch arm gates its announcement on the commit");
    assert!(
        source[batch_commit..].find("log_active_once(").is_some(),
        "the batch announcement must follow its commit"
    );
    let single_ok = source
        .find("Ok(wrote) => {")
        .expect("the single arm gates on what upsert_one reported");
    let wrote_gate = source[single_ok..]
        .find("if wrote {")
        .expect("the single arm must check that something was written");
    let single_announce = source[single_ok..]
        .find("log_active_once(")
        .expect("the single arm announces");
    assert!(
        single_announce > wrote_gate,
        "an empty merge must not announce the cache as active"
    );
}
