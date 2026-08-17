use super::*;

/// `MOON_FIXTURE_DIR` is process-wide, so tests that write it and tests that read it must not run
/// at the same time inside this one test binary.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Process-scoped scratch directory; `tag` separates tests that share one process.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moon-fixture-{}-{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Write a candle database holding exactly the given `(exchange, market, kind)` rows.
fn seed_klines(path: &Path, rows: &[(&str, &str, u32)]) {
    let conn = rusqlite::Connection::open(path).expect("open");
    conn.execute_batch(
        "CREATE TABLE chunks(exchange TEXT NOT NULL, market TEXT NOT NULL, kind INTEGER NOT NULL,
             day INTEGER NOT NULL, rows BLOB NOT NULL, updated_ms INTEGER NOT NULL,
             PRIMARY KEY(exchange, market, kind, day));",
    )
    .expect("schema");
    for (exchange, market, kind) in rows {
        conn.execute(
            "INSERT INTO chunks(exchange, market, kind, day, rows, updated_ms)
             VALUES(?1, ?2, ?3, 1, x'00', 0)",
            rusqlite::params![exchange, market, kind],
        )
        .expect("seed");
    }
}

/// Pack one candle row the way `kline_cache` stores it.
fn packed_row(offset_ms: u32, close: f32) -> Vec<u8> {
    let mut row = Vec::with_capacity(ROW_BYTES);
    row.extend_from_slice(&offset_ms.to_le_bytes());
    for value in [close, close, close, close, 0.0f32] {
        row.extend_from_slice(&value.to_le_bytes());
    }
    row
}

/// The base kind must be the finest one that still divides the request: below 5 minutes only the
/// 1-minute base fits, and from 5 minutes up the deeper 5-minute base is preferred.
#[test]
fn base_kind_follows_the_requested_timeframe() {
    assert_eq!(base_kind_for(60_000), 1);
    assert_eq!(base_kind_for(180_000), 1);
    assert_eq!(base_kind_for(300_000), 5);
    assert_eq!(base_kind_for(1_800_000), 5);
    assert_eq!(base_kind_for(86_400_000), 5);
}

/// The name is joined into a path that is then REMOVED recursively, so anything that could climb
/// out of the temporary root has to be refused.
#[test]
fn name_validation_refuses_path_traversal() {
    assert!(validate_name("chart-ace").is_ok());
    assert!(validate_name("chart_ace.2").is_ok());
    assert!(validate_name("..").is_err());
    assert!(validate_name("../../etc").is_err());
    assert!(validate_name("..\\..\\windows").is_err());
    assert!(validate_name("a/b").is_err());
    assert!(validate_name("").is_err());
}

/// A candle database carrying two markets is a broken bench, not a bench with a choice: its trades
/// belong to one market and would be drawn on the other one's prices.
#[test]
fn describe_rejects_a_mixed_fixture() {
    let dir = scratch("mixed");
    let path = dir.join("klines.sqlite");
    seed_klines(&path, &[("200:00000000", "ACEUSDT", 1), ("200:00000000", "BTCUSDT", 1)]);

    let err = describe(&path).expect_err("two markets must fail");
    assert!(
        err.to_string().contains("ожидается ровно одна"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The pair is read from the data, so a fixture for another coin needs no code change.
#[test]
fn describe_reads_the_single_pair() {
    let dir = scratch("single");
    let path = dir.join("klines.sqlite");
    seed_klines(&path, &[("200:00000000", "ACEUSDT", 1), ("200:00000000", "ACEUSDT", 5)]);

    let (exchange, market) = describe(&path).expect("single pair");
    assert_eq!(exchange, "200:00000000");
    assert_eq!(market, "ACEUSDT");
    let _ = std::fs::remove_dir_all(&dir);
}

/// An empty candle database would open a chart with no price at all.
#[test]
fn describe_rejects_an_empty_fixture() {
    let dir = scratch("empty");
    let path = dir.join("klines.sqlite");
    seed_klines(&path, &[]);

    assert!(describe(&path).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Shifting is what puts the bench under the chart's live edge, and it must (1) move rows by the
/// exact offset, (2) regroup them into the day they now belong to, and (3) drop what would land
/// after "now" — the fixture's candles run past its trades, and that surplus would otherwise be
/// drawn to the right of the current moment.
#[test]
fn shift_moves_regroups_and_drops_the_future() {
    let dir = scratch("shift");
    let path = dir.join("klines.sqlite");
    let conn = rusqlite::Connection::open(&path).expect("open");
    conn.execute_batch(
        "CREATE TABLE chunks(exchange TEXT NOT NULL, market TEXT NOT NULL, kind INTEGER NOT NULL,
             day INTEGER NOT NULL, rows BLOB NOT NULL, updated_ms INTEGER NOT NULL,
             PRIMARY KEY(exchange, market, kind, day));",
    )
    .expect("schema");
    // Day 10, two rows: one early, one late enough that a 2-hour shift moves it into day 11.
    let mut blob = packed_row(0, 1.0);
    blob.extend_from_slice(&packed_row(23 * 3_600_000, 2.0));
    conn.execute(
        "INSERT INTO chunks VALUES('200:00000000','ACEUSDT',1,10,?1,0)",
        [&blob],
    )
    .expect("seed");
    drop(conn);

    let shift_ms = 2 * 3_600_000;
    // "Now" sits between the two shifted rows, so the later one must be dropped.
    let now_ms = 10 * DAY_MS + shift_ms + 3_600_000;
    shift_candles(&path, shift_ms, now_ms).expect("shift");

    let conn = rusqlite::Connection::open(&path).expect("reopen");
    let rows: Vec<(i64, Vec<u8>)> = conn
        .prepare("SELECT day, rows FROM chunks ORDER BY day")
        .expect("prepare")
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    assert_eq!(rows.len(), 1, "the future row must be dropped");
    let (day, blob) = &rows[0];
    assert_eq!(*day, 10, "an in-day shift keeps the row in its day");
    assert_eq!(blob.len(), ROW_BYTES, "exactly one row survives");
    let offset = u32::from_le_bytes(blob[0..4].try_into().expect("4 bytes"));
    assert_eq!(offset as i64, shift_ms, "the row moved by the shift");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A shift that crosses midnight must re-key the row into the following day, or the chunk's
/// in-day offset would overflow the day it is filed under.
#[test]
fn shift_across_midnight_rekeys_the_day() {
    let dir = scratch("midnight");
    let path = dir.join("klines.sqlite");
    let conn = rusqlite::Connection::open(&path).expect("open");
    conn.execute_batch(
        "CREATE TABLE chunks(exchange TEXT NOT NULL, market TEXT NOT NULL, kind INTEGER NOT NULL,
             day INTEGER NOT NULL, rows BLOB NOT NULL, updated_ms INTEGER NOT NULL,
             PRIMARY KEY(exchange, market, kind, day));",
    )
    .expect("schema");
    conn.execute(
        "INSERT INTO chunks VALUES('200:00000000','ACEUSDT',1,10,?1,0)",
        [&packed_row(23 * 3_600_000, 5.0)],
    )
    .expect("seed");
    drop(conn);

    let shift_ms = 2 * 3_600_000;
    shift_candles(&path, shift_ms, i64::MAX).expect("shift");

    let conn = rusqlite::Connection::open(&path).expect("reopen");
    let (day, blob): (i64, Vec<u8>) = conn
        .query_row("SELECT day, rows FROM chunks", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .expect("row");
    assert_eq!(day, 11, "the row moved into the next day");
    let offset = u32::from_le_bytes(blob[0..4].try_into().expect("4 bytes"));
    assert_eq!(offset, 3_600_000, "and carries its in-day offset");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The last close feeds the synthetic tick stream's starting price; reading it from the newest
/// chunk is what keeps the ticker, the order book and the chart on the same order of magnitude.
#[test]
fn newest_close_reads_the_last_row() {
    let dir = scratch("close");
    let path = dir.join("klines.sqlite");
    let conn = rusqlite::Connection::open(&path).expect("open");
    conn.execute_batch(
        "CREATE TABLE chunks(exchange TEXT NOT NULL, market TEXT NOT NULL, kind INTEGER NOT NULL,
             day INTEGER NOT NULL, rows BLOB NOT NULL, updated_ms INTEGER NOT NULL,
             PRIMARY KEY(exchange, market, kind, day));",
    )
    .expect("schema");
    let mut blob = packed_row(0, 1.5);
    blob.extend_from_slice(&packed_row(60_000, 0.19538));
    conn.execute(
        "INSERT INTO chunks VALUES('200:00000000','ACEUSDT',1,10,?1,0)",
        [&blob],
    )
    .expect("seed");
    conn.execute(
        "INSERT INTO chunks VALUES('200:00000000','ACEUSDT',1,9,?1,0)",
        [&packed_row(0, 99.0)],
    )
    .expect("seed old");
    drop(conn);

    let close = newest_close(&path).expect("close");
    assert!(
        (close - 0.19538).abs() < 1e-6,
        "expected the newest row's close, got {close}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `MOON_FIXTURE_DIR` may name either the set itself or the directory holding the sets: both
/// readings are natural, and guessing wrong yields a confusing "fixture not found".
#[test]
fn locate_accepts_the_set_or_its_parent() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("locate");
    let set = dir.join("chart-ace");
    std::fs::create_dir_all(&set).expect("create set");

    std::env::set_var("MOON_FIXTURE_DIR", &dir);
    assert_eq!(locate("chart-ace").as_deref(), Some(set.as_path()));
    std::env::set_var("MOON_FIXTURE_DIR", &set);
    assert_eq!(locate("chart-ace").as_deref(), Some(set.as_path()));
    std::env::set_var("MOON_FIXTURE_DIR", &dir);
    assert_eq!(locate("chart-btc"), None);
    std::env::remove_var("MOON_FIXTURE_DIR");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The committed set must stay loadable: the whole point is that every machine opens the same
/// bench, so a truncated or re-keyed database is a defect the test suite has to see.
#[test]
fn committed_fixture_is_readable() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(dir) = locate("chart-ace") else {
        // A source checkout always has it; a packaged build may not ship `fixtures/`.
        return;
    };
    let (exchange, market) = describe(&dir.join("klines.sqlite")).expect("candle pair");
    assert_eq!(market, "ACEUSDT");
    assert!(exchange.starts_with("200:"), "exchange key: {exchange}");
    let close = newest_close(&dir.join("klines.sqlite")).expect("newest close");
    assert!(close > 0.0, "bench must carry a usable last price");

    let conn = rusqlite::Connection::open(dir.join("reports.sqlite")).expect("open reports");
    let (rows, cores, coins): (i64, i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COUNT(DISTINCT core_uid), COUNT(DISTINCT coin) FROM orders_rep",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("count");
    assert!(rows > 100, "bench should carry a dense day, got {rows} trades");
    assert_eq!(cores, 1, "the bench is one core");
    assert_eq!(coins, 1, "the bench is one coin");

    // The bench's core uid must be the one the plaintext synthetic core is given, or the chart
    // would open on a core whose closed trades are filed under a different id and show none.
    let core_uid: i64 = conn
        .query_row("SELECT DISTINCT core_uid FROM orders_rep", [], |r| r.get(0))
        .expect("core uid");
    assert_eq!(core_uid, 1, "plaintext core runs as uid 1");

    // Anonymisation is part of the committed artifact: the generator replaced strategy names, and
    // a regenerated fixture that forgets to must not reach a public repository unnoticed.
    let leaked: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM orders_rep
             WHERE channelname LIKE '%Spread%' OR sellreason LIKE '%Spread%'",
            [],
            |r| r.get(0),
        )
        .expect("leak scan");
    assert_eq!(leaked, 0, "strategy names leaked into the fixture");
}
