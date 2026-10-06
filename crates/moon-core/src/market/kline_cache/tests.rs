use super::*;
use crate::market::candles::estimate_quote_volume;

/// Owns an isolated synthetic fixture; retries cleanup while cache workers close their files.
struct BenchDir(std::path::PathBuf);

impl BenchDir {
    /// Creates a unique directory without sharing files between ignored benchmarks.
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{name}-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&path).expect("fixture directory");
        Self(path)
    }
}

impl Drop for BenchDir {
    /// Removes only this fixture after the worker's asynchronous connection shutdown.
    fn drop(&mut self) {
        for _ in 0..50 {
            if std::fs::remove_dir_all(&self.0).is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("fixture cleanup failed: {}", self.0.display());
    }
}

/// Builds native-cadence candles through the normal merge path, including expired days.
fn bench_kline_fixture(dir: &BenchDir) -> (KlineCache, std::path::PathBuf, i64) {
    let path = dir.0.join("synthetic.sqlite");
    let cache = KlineCache::open(path.clone()).expect("fixture cache");
    let today = now_unix_ms() / DAY_MS * DAY_MS;
    let setup = std::time::Instant::now();
    for market in 0..300 {
        let items = [1, 5, 60]
            .into_iter()
            .map(|kind_min| MergeItem {
                exchange: "synx".into(),
                market: format!("SYN{market:04}-USDT"),
                kind_min,
                rows: (0..120)
                    .flat_map(|day| {
                        (0..1440 / kind_min).map(move |slot| {
                            candle(
                                (today - day * DAY_MS + i64::from(slot * kind_min) * 60_000) as f64,
                                100.0,
                            )
                        })
                    })
                    .collect(),
            })
            .collect();
        cache.merge_batch_blocking(items);
    }
    let rows = cache
        .read_range("synx", "SYN0000-USDT", 60, today, today + DAY_MS)
        .expect("fixture read");
    assert!(!rows.is_empty());
    let conn = rusqlite::Connection::open(&path).expect("checkpoint connection");
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .expect("checkpoint");
    println!(
        "[bench] kline_fixture markets=300 kinds=3 days=120 candles=63072000 native_cadence=true setup_ms={:.3}",
        setup.elapsed().as_secs_f64() * 1e3
    );
    (cache, path, today)
}

/// Measures caller-side populated-cache open, with startup work preceding the untimed sanity read.
#[test]
#[ignore = "synthetic performance baseline"]
fn bench_kline_open_latency() {
    let dir = BenchDir::new("bench-kline-open");
    let (seed, path, today) = bench_kline_fixture(&dir);
    let mut handles = Vec::new();
    let mut total = std::time::Duration::ZERO;
    for repetition in 0..3 {
        let copy = dir.0.join(format!("open-{repetition}.sqlite"));
        std::fs::copy(&path, &copy).expect("fresh populated copy");
        let started = std::time::Instant::now();
        let cache = KlineCache::open(copy).expect("populated open");
        let elapsed = started.elapsed();
        total += elapsed;
        // An acknowledged batch with no rows waits for startup pruning without changing the file.
        // The public read timeout stays short even when pruning the full fixture takes seconds.
        cache.merge_batch_blocking(vec![MergeItem {
            exchange: "synx".into(),
            market: "SYN0000-USDT".into(),
            kind_min: 60,
            rows: Vec::new(),
        }]);
        assert!(
            !cache
                .read_range("synx", "SYN0000-USDT", 60, today, today + DAY_MS)
                .expect("opened read")
                .is_empty()
        );
        println!(
            "[bench] bench_kline_open_latency repetition={repetition} open_ms={:.3}",
            elapsed.as_secs_f64() * 1e3
        );
        handles.push(cache);
    }
    println!(
        "[bench] bench_kline_open_latency mean_ms={:.3}",
        total.as_secs_f64() * 1e3 / 3.0
    );
    drop(handles);
    drop(seed);
}

/// Measures an acknowledged tail rewrite so queue submission alone cannot look like a speedup.
#[test]
#[ignore = "synthetic performance baseline"]
fn bench_kline_merge_tail_batch() {
    let dir = BenchDir::new("bench-kline-tail");
    let (cache, _, today) = bench_kline_fixture(&dir);
    let mut total = std::time::Duration::ZERO;
    for repetition in 0..3 {
        let items = (0..300)
            .flat_map(|market| {
                [1, 5, 60].into_iter().map(move |kind_min| MergeItem {
                    exchange: "synx".into(),
                    market: format!("SYN{market:04}-USDT"),
                    kind_min,
                    rows: (0..1440 / kind_min)
                        .map(|slot| {
                            candle((today + i64::from(slot * kind_min) * 60_000) as f64, 100.0)
                        })
                        .collect(),
                })
            })
            .collect();
        let started = std::time::Instant::now();
        cache.merge_batch_blocking(items);
        let elapsed = started.elapsed();
        total += elapsed;
        println!(
            "[bench] bench_kline_merge_tail_batch repetition={repetition} merge_ms={:.3}",
            elapsed.as_secs_f64() * 1e3
        );
    }
    assert!(
        !cache
            .read_range("synx", "SYN0299-USDT", 5, today, today + DAY_MS)
            .expect("tail read")
            .is_empty()
    );
    println!(
        "[bench] bench_kline_merge_tail_batch mean_ms={:.3}",
        total.as_secs_f64() * 1e3 / 3.0
    );
    drop(cache);
}

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

/// `kline_cache.rs:legacy_volume_is_quote` widening its coarse-kind gate to kinds 1 or 5
/// reinterprets ambiguous cached base rows as quote money, silently under-reading futures history.
#[test]
fn legacy_quote_reinterpretation_is_limited_to_proven_coarse_binance_futures_rows() {
    let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
    init_schema(&conn).expect("v2 schema");
    let day = 20_002 * DAY_MS;
    let row = ChartCandle {
        t_open_ms: day as f64,
        open: 90.0,
        high: 120.0,
        low: 80.0,
        close: 110.0,
        volume: 12_000.0,
        quote_volume: 0.0,
    };
    let blob = packed_rows(&[row], day, false);
    assert_eq!(blob.len(), ROW_BYTES_V1, "the test supplies one v1 row");

    for (exchange, kind, expected_quote) in [
        ("4:00000000", 1440, 12_000.0),
        ("4:00000000", 1, 1_200_000.0),
        ("4:00000000", 5, 1_200_000.0),
        ("3:00000000", 1440, 1_200_000.0),
        ("200:00000000", 1440, 1_200_000.0),
        ("x", 1440, 1_200_000.0),
    ] {
        conn.execute(
            "INSERT INTO chunks(exchange, market, kind, day, rows, updated_ms) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![exchange, "PAIR", kind, day / DAY_MS, &blob, 0],
        )
        .expect("legacy row");
        let decoded = read_rows(&conn, exchange, "PAIR", kind, day, day).expect("legacy read");
        assert_eq!(decoded.len(), 1);
        assert_eq!(
            decoded[0].quote_volume, expected_quote,
            "{exchange} kind {kind}"
        );
    }
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

/// `market/kline_cache.rs:prune_expired` retaining only `chunks` lets `chunks_v2` grow forever,
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
    prune_expired(&conn);
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
    let rows = [
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

/// An expired queued read runs no query and drops its reply; the following read runs one query.
#[test]
fn kline_expired_read_runs_no_query() {
    let conn = rusqlite::Connection::open_in_memory().expect("deadline database");
    init_schema(&conn).expect("schema");
    let day = now_unix_ms() / DAY_MS * DAY_MS;
    upsert_one(
        &conn,
        "synx",
        "SYN0000-USDT",
        1,
        &[candle(day as f64, 10.0)],
        1,
    )
    .expect("seed candle");
    let (tx, rx) = mpsc::channel();
    let pruned = Arc::new((Mutex::new(false), Condvar::new()));
    let worker_pruned = Arc::clone(&pruned);
    let worker = std::thread::spawn(move || {
        COUNT_READ_QUERIES.with(|count| count.set(true));
        run(conn, rx, worker_pruned);
    });
    let cache = KlineCache { tx, pruned };
    let before = READ_QUERIES.load(Ordering::Relaxed);
    let (reply, expired) = mpsc::channel();
    cache
        .tx
        .send(Op::Read {
            exchange: "synx".into(),
            market: "SYN0000-USDT".into(),
            kind_min: 1,
            from_ms: day,
            to_ms: day + DAY_MS,
            deadline: Instant::now() - Duration::from_secs(1),
            reply,
        })
        .expect("expired request");
    let rows = cache
        .read_range("synx", "SYN0000-USDT", 1, day, day + DAY_MS)
        .expect("normal request");
    assert_eq!(rows, vec![candle(day as f64, 10.0)]);
    assert_eq!(READ_QUERIES.load(Ordering::Relaxed) - before, 1);
    assert!(matches!(
        expired.recv_timeout(READ_TIMEOUT),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
    drop(cache);
    worker.join().expect("worker shutdown");
}

/// Moving pruning behind the first receive would expose expired candles to the first chart read.
#[test]
fn kline_open_then_read_sees_pruned_rows() {
    let dir = BenchDir::new("kline-startup-retention");
    let path = dir.0.join("synthetic.sqlite");
    let today = now_unix_ms() / DAY_MS;
    let expired = today - retention_days(5) - 1;
    {
        let conn = rusqlite::Connection::open(&path).expect("fixture database");
        init_schema(&conn).expect("schema");
        for (table, market, quote) in [
            ("chunks", "SYN0000-USDT", false),
            ("chunks_v2", "SYN0001-USDT", true),
        ] {
            for day in [expired, today] {
                let row = candle((day * DAY_MS) as f64, 10.0);
                conn.execute(
                    &format!("INSERT INTO {table}(exchange, market, kind, day, rows, updated_ms) VALUES(?1, ?2, ?3, ?4, ?5, ?6)"),
                    rusqlite::params!["synx", market, 5, day, packed_rows(&[row], day * DAY_MS, quote), 1],
                ).expect("seed retention row");
            }
        }
    }
    let cache = KlineCache::open(path).expect("cache open");
    for market in ["SYN0000-USDT", "SYN0001-USDT"] {
        let rows = cache
            .read_range("synx", market, 5, expired * DAY_MS, (today + 1) * DAY_MS)
            .expect("startup read");
        assert_eq!(
            rows.len(),
            1,
            "{market} must contain only the current candle"
        );
        assert_eq!(rows[0].t_open_ms, (today * DAY_MS) as f64);
        assert_eq!(rows[0].open, 10.0);
    }
    drop(cache);
}

/// Statement reuse must preserve day grouping, incoming-row precedence and per-key isolation.
#[test]
fn kline_merge_equivalence() {
    let dir = BenchDir::new("kline-merge-equivalence");
    let cache = KlineCache::open(dir.0.join("synthetic.sqlite")).expect("cache open");
    let day = now_unix_ms() / DAY_MS * DAY_MS;
    for market in ["SYN0000-USDT", "SYN0001-USDT"] {
        for kind_min in [1, 5] {
            let price = if market == "SYN0000-USDT" {
                10.0
            } else {
                100.0
            } + kind_min as f32;
            for rows in [
                vec![
                    candle((day - 60_000) as f64, price),
                    candle(day as f64, price + 1.0),
                ],
                vec![
                    candle(day as f64, price + 2.0),
                    candle((day + DAY_MS) as f64, price + 3.0),
                ],
                vec![
                    candle((day - 60_000) as f64, price),
                    candle(day as f64, price + 2.0),
                ],
            ] {
                cache.merge_batch_blocking(vec![MergeItem {
                    exchange: "synx".into(),
                    market: market.into(),
                    kind_min,
                    rows,
                }]);
            }
        }
    }
    for (market, kind, prices) in [
        ("SYN0000-USDT", 1, [11.0, 13.0, 14.0]),
        ("SYN0000-USDT", 5, [15.0, 17.0, 18.0]),
        ("SYN0001-USDT", 1, [101.0, 103.0, 104.0]),
        ("SYN0001-USDT", 5, [105.0, 107.0, 108.0]),
    ] {
        let expected = vec![
            candle((day - 60_000) as f64, prices[0]),
            candle(day as f64, prices[1]),
            candle((day + DAY_MS) as f64, prices[2]),
        ];
        assert_eq!(
            cache
                .read_range("synx", market, kind, day - DAY_MS, day + 2 * DAY_MS)
                .expect("merged read"),
            expected,
            "{market} kind {kind}"
        );
    }
    drop(cache);
}

/// Starting the reply timeout before startup retention completes loses a valid first-read answer.
#[test]
fn kline_read_during_prune_waits_for_rows() {
    let dir = BenchDir::new("kline-delayed-prune");
    let path = dir.0.join("synthetic.sqlite");
    let today = now_unix_ms() / DAY_MS * DAY_MS;
    let expired = today - (retention_days(5) + 1) * DAY_MS;
    {
        let conn = rusqlite::Connection::open(&path).expect("fixture database");
        init_schema(&conn).expect("schema");
        upsert_one(
            &conn,
            "synx",
            "SYN0000-USDT",
            5,
            &[candle(expired as f64, 10.0), candle(today as f64, 12.0)],
            1,
        )
        .expect("seed expired and current rows");
    }
    let _delay_reset = PruneDelayReset;
    PRUNE_DELAY_MS.store(500, Ordering::Relaxed);
    DELAY_PRUNE_FOR_TEST.with(|delay| delay.set(true));
    let cache = KlineCache::open(path).expect("cache open");
    DELAY_PRUNE_FOR_TEST.with(|delay| delay.set(false));
    assert!(
        !*cache.pruned.0.lock().expect("startup flag"),
        "exercise the startup window"
    );
    let rows = cache.read_range("synx", "SYN0000-USDT", 5, expired, today + DAY_MS);
    assert_eq!(rows, Some(vec![candle(today as f64, 12.0)]));
    drop(cache);
}

/// Restores the test-only startup delay and opt-in even if a fixture assertion unwinds.
struct PruneDelayReset;

impl Drop for PruneDelayReset {
    /// Prevents a failed test from leaving later workers delayed.
    fn drop(&mut self) {
        PRUNE_DELAY_MS.store(0, Ordering::Relaxed);
        DELAY_PRUNE_FOR_TEST.with(|delay| delay.set(false));
    }
}

/// Omitting unwind publication leaves synchronous readers blocked after a retention panic.
#[test]
fn prune_flag_guard_releases_on_panic() {
    let pruned = Arc::new((Mutex::new(false), Condvar::new()));
    let result = std::panic::catch_unwind(|| {
        let _guard = PrunedOnDrop::new(Arc::clone(&pruned));
        panic!("synthetic retention panic");
    });
    assert!(result.is_err());
    assert!(*pruned.0.lock().expect("startup flag"));
}
