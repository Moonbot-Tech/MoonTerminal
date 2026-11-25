use super::*;
use crate::market::candles::{ChartCandle, estimate_quote_volume};

/// The real pre-repair cache slice: chunks cut byte for byte on 2026-11-25 from a live
/// `klines.sqlite` written by v0.69.2 — Bybit BTCUSDT and OKX BTC-USDT-SWAP 5-minute days that
/// hold both the bad deep rows and the right trade-folded ones, their ring days, a Bybit
/// one-minute day, Binance Futures (code 4) days, three Gate Spot daily rows, and the unknown code
/// 221's 5-minute and ring days, plus two legacy v1 rows.
const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/fixtures/kline-repair/klines.sqlite"
);

/// A private copy of [`FIXTURE`], opened the way the cache opens its file, removed on drop.
struct Copy {
    path: std::path::PathBuf,
    conn: Option<Connection>,
}

impl Copy {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "kline-repair-{name}-{}-{stamp}.sqlite",
            std::process::id()
        ));
        std::fs::copy(FIXTURE, &path).expect("fixture copy");
        let conn = Connection::open(&path).expect("open copy");
        super::super::init_schema(&conn).expect("schema of an opening build");
        Self {
            path,
            conn: Some(conn),
        }
    }

    fn conn(&self) -> &Connection {
        self.conn.as_ref().expect("open")
    }
}

impl Drop for Copy {
    fn drop(&mut self) {
        drop(self.conn.take());
        for suffix in ["", "-wal", "-shm"] {
            let mut p = self.path.clone().into_os_string();
            p.push(suffix);
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Every row of one key, as the cache reads it.
fn rows(conn: &Connection, exchange: &str, market: &str, kind: u32) -> Vec<ChartCandle> {
    super::super::read_rows(
        conn,
        exchange,
        market,
        kind,
        0,
        40_000 * super::super::DAY_MS,
    )
    .expect("read")
}

/// Every stored blob, keyed, for byte-for-byte comparisons.
fn blobs(conn: &Connection) -> Vec<(String, String, u32, i64, Vec<u8>)> {
    let mut stmt = conn
        .prepare("SELECT exchange, market, kind, day, rows FROM chunks_v2 ORDER BY 1, 2, 3, 4")
        .expect("prepare");
    stmt.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })
    .expect("query")
    .collect::<rusqlite::Result<_>>()
    .expect("rows")
}

fn median(mut v: Vec<f32>) -> f32 {
    assert!(!v.is_empty(), "median of nothing");
    v.sort_by(f32::total_cmp);
    v[v.len() / 2]
}

/// The median 5-minute turnover of a market's candles folded from trades: the rows whose quote is
/// not the OHLC4 estimate of their volume, because the recorder sums `price * qty` itself. The
/// test's oracle, independent of the repair and of the core's figure.
fn trade_folded_quote(rows: &[ChartCandle]) -> f32 {
    median(
        rows.iter()
            .filter(|c| {
                c.quote_volume != estimate_quote_volume(c.volume, c.open, c.high, c.low, c.close)
            })
            .map(|c| c.quote_volume)
            .collect(),
    )
}

/// The old cache, repaired, reads ring volumes in the range the same market's trades give, and the
/// rows the repair cannot fix are gone while the rows it must not touch are byte-for-byte intact.
///
/// By the numbers, Bybit BTCUSDT: its trade-folded 5-minute candles turn over about 10 M $; the
/// stored ring rows claimed some 7.7e11 $ (the core's turnover times the price) before the repair,
/// and the core's own figure — the same order as the trades — after it.
///
/// Breakage: the ring left unrepaired (the scale in billions), the stored quote taken instead of
/// the stored volume (still billions), Binance Futures' already-right rows rewritten (their quote
/// turned into a base estimate, a price times too small), the mixed kinds 1 and 5 kept, or an
/// unknown venue's deep rows reinterpreted although they are base.
#[test]
fn the_old_cache_reads_trade_sized_volumes_after_the_repair() {
    let copy = Copy::new("sane");
    let conn = copy.conn();
    let markets = [("2:00000000", "BTCUSDT"), ("15:00000000", "BTC-USDT-SWAP")];
    let mut oracle = Vec::new();
    let mut ring_before = Vec::new();
    for (ex, mkt) in markets {
        let trades = trade_folded_quote(&rows(conn, ex, mkt, 5));
        let ring = rows(conn, ex, mkt, RING_5M_KIND);
        assert!(
            median(ring.iter().map(|c| c.quote_volume).collect()) > trades * 1_000.0,
            "{ex}: the fixture holds the bad ring rows"
        );
        oracle.push(trades);
        ring_before.push(ring);
    }
    let untouched_before: Vec<_> = blobs(conn)
        .into_iter()
        .filter(|(ex, _, kind, _, _)| {
            ex.starts_with("4:") || (ex.starts_with("221:") && *kind != RING_5M_KIND)
        })
        .collect();
    let other_ring_before = rows(conn, "221:00000000", "BTCUSDT", RING_5M_KIND);
    let coarse_before = rows(conn, "8:00000000", "HOD_USDT", 1440);

    let report = apply(conn).expect("repair").expect("first application");
    assert_eq!(
        report,
        Report {
            // Bybit and OKX kind 5, Bybit kind 1.
            deleted_chunks: 3,
            // The rings of Bybit, OKX and 221, and Gate Spot's three daily rows.
            rewritten_chunks: 6,
        }
    );

    for (((ex, mkt), trades), before) in markets.into_iter().zip(oracle).zip(ring_before) {
        let after = rows(conn, ex, mkt, RING_5M_KIND);
        assert_eq!(after.len(), before.len(), "{ex}: no ring row lost");
        for (b, a) in before.iter().zip(&after) {
            assert_eq!(
                a.quote_volume, b.volume,
                "{ex}: the stored volume is the quote"
            );
            assert_eq!(
                (a.open, a.high, a.low, a.close),
                (b.open, b.high, b.low, b.close)
            );
        }
        let ring = median(after.iter().map(|c| c.quote_volume).collect());
        assert!(
            ring > trades / 4.0 && ring < trades * 4.0,
            "{ex}: ring {ring} against trades {trades}"
        );
        assert!(rows(conn, ex, mkt, 5).is_empty(), "{ex}: kind 5 deleted");
        assert!(rows(conn, ex, mkt, 1).is_empty(), "{ex}: kind 1 deleted");
    }
    for (b, a) in other_ring_before
        .iter()
        .zip(&rows(conn, "221:00000000", "BTCUSDT", RING_5M_KIND))
    {
        assert_eq!(
            a.quote_volume, b.volume,
            "an unknown venue's ring is quote too"
        );
    }
    for (b, a) in coarse_before
        .iter()
        .zip(&rows(conn, "8:00000000", "HOD_USDT", 1440))
    {
        assert_eq!(a.quote_volume, b.volume, "a coarse deep row is quote");
    }
    let untouched_after: Vec<_> = blobs(conn)
        .into_iter()
        .filter(|(ex, _, kind, _, _)| {
            ex.starts_with("4:") || (ex.starts_with("221:") && *kind != RING_5M_KIND)
        })
        .collect();
    assert_eq!(untouched_after, untouched_before);
}

/// The repair applies once: a second open — also after an older build has opened the file and
/// set `user_version` back — changes nothing.
///
/// Breakage: the mark kept in `user_version` (an older build resets it and the next open rewrites
/// the ring again, reading the fixed rows' base estimate as turnover — a price times too small), or
/// no mark at all.
#[test]
fn the_repair_applies_once() {
    let copy = Copy::new("once");
    let conn = copy.conn();
    assert!(apply(conn).expect("repair").is_some());
    let fixed = blobs(conn);
    super::super::init_schema(conn).expect("an older build opening the file");
    assert_eq!(apply(conn).expect("second open"), None);
    assert_eq!(blobs(conn), fixed);
}

/// The action table of the module doc.
///
/// Breakage: code 4's right rows touched, an unknown venue's base deep rows reinterpreted, the
/// mixed kinds rewritten instead of deleted, a key the cache did not write (the fixture's private
/// bench copy, a test key) acted on, or the live predicate consulted (a venue a later build adds
/// rewritten from rows that were never wrong).
#[test]
fn each_kind_gets_the_action_its_producers_allow() {
    use Action::*;
    for (exchange, kind, want) in [
        ("4:00000000", RING_5M_KIND, Keep),
        ("4:00000000", 5, Keep),
        ("4:00000000", 1440, Keep),
        ("2:00000000", RING_5M_KIND, Reinterpret),
        ("2:00000000", 1, Delete),
        ("2:00000000", 5, Delete),
        ("2:00000000", 30, Reinterpret),
        ("15:0a0b0c0d", 1440, Reinterpret),
        ("3:00000000", 60, Reinterpret),
        ("2:00000000", 0, Keep),
        ("221:00000000", RING_5M_KIND, Reinterpret),
        ("221:00000000", 5, Keep),
        ("221:00000000", 1440, Keep),
        // A venue a later build may name: its timeframe rows were right under their day's rule.
        ("16:00000000", 5, Keep),
        ("16:00000000", 1440, Keep),
        ("200:00000000", 1, Keep),
        ("synx", RING_5M_KIND, Keep),
    ] {
        assert_eq!(action(exchange, kind), want, "{exchange} kind {kind}");
    }
}

/// How long the repair takes on a whole real cache, which it rewrites in place: run by hand on a
/// COPY, `MOON_KLINE_REPAIR_BENCH=<copy of klines.sqlite> cargo test -p moon-core --target
/// x86_64-pc-windows-msvc -- --ignored repair_timing`. Its length decides whether the first open's
/// readers may wait for it.
#[test]
#[ignore = "needs a copy of a real cache; it rewrites the file"]
fn repair_timing_on_a_real_cache() {
    let Ok(path) = std::env::var("MOON_KLINE_REPAIR_BENCH") else {
        return;
    };
    let conn = Connection::open(&path).expect("open the copy");
    super::super::init_schema(&conn).expect("schema");
    let started = std::time::Instant::now();
    let report = apply(&conn).expect("repair");
    println!(
        "[bench] repair {:?} in {} ms",
        report,
        started.elapsed().as_millis()
    );
}
