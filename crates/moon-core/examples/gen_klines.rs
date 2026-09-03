//! Deterministic `klines.sqlite` seeder for the report-replica measurement fixture
//! (`tools/gen_replica.py`), built through `KlineCache`'s own production write API rather than a
//! second binary-codec implementation.
//!
//! `chunks_v2`'s packed row codec (`pack_rows_v2`) is private to `market/kline_cache.rs` on
//! purpose: a Python re-implementation of it would be a second binary-format authority no drift
//! check could prove equivalent to the first (rejected in review). This helper instead opens the
//! cache and calls `KlineCache::merge_batch_blocking` directly, the same entry point the
//! background recorder uses.
//!
//! Usage:
//!     cargo run --release -p moon-core --example gen_klines -- <data_dir> <span_days> [seed]
//!
//! `KlineCache::open` prunes chunks older than each kind's retention window
//! (`retention_days`: 30 days for 1-minute, 15 for 5-minute, 3650 for everything coarser) the
//! moment it opens the database — including on a LATER reopen by a reader, not just this
//! process's own. Anchoring the generated series at a fixed historical timestamp (as
//! `gen_replica.py` does for `reports.sqlite`) would therefore make the 1- and 5-minute series
//! vanish the next time anything opens the cache, days after generation. To guarantee
//! `KlineCache::read_range` stays non-empty for every kind this fixture writes, every series here
//! is anchored at the REAL wall clock (`now_unix_ms`) instead: `klines.sqlite` is consequently the
//! one fixture file that is NOT byte-identical across reruns on different days, unlike
//! `reports.sqlite` / `strategies.sqlite` / `valuation.sqlite` (see the generator's own report for
//! why this is a deliberate deviation from the frozen contract's byte-identical clause).

use std::path::PathBuf;

use moon_core::market::ChartCandle;
use moon_core::market::kline_cache::{KlineCache, MergeItem};

const DAY_MS: i64 = 86_400_000;
const KINDS: [u32; 3] = [1, 5, 60];
const MARKETS: usize = 50;
// FBinance (4) and ByBit (7) codes, per feed::types::ExchangeId's doc comment; dex=0 (no HIP-3
// DEX), formatted exactly as market/source/read.rs's `exchange_key` does: `"{code}:{dex:08x}"`.
const EXCHANGE_CODES: [u8; 2] = [4, 7];
// A handful of markets also get direct legacy `chunks` (v1) rows for one day, so `read_rows`'
// v1/v2 merge is exercised — never through a re-implemented v2 codec, only the documented
// 24-byte-per-row v1 layout (`u32 offset_ms + 5×f32`, no turnover; kline_cache.rs's module doc).
const LEGACY_MARKETS: usize = 5;

/// Deterministic splitmix64 source for reproducible candle values despite the real-time anchor.
struct Rng(u64);

impl Rng {
    /// Advance the generator and return its next uniformly distributed `u64`.
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Return the next generator value scaled to the half-open unit interval.
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Return the next generator value scaled to the requested half-open numeric interval.
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + self.next_f64() * (hi - lo)
    }
}

/// Build one synthetic market identity from its deterministic fixture ordinal.
fn market_name(i: usize) -> String {
    format!("SYN{i:03}USDT")
}

/// Build the persisted exchange key assigned to one synthetic market ordinal.
fn exchange_key(i: usize) -> String {
    format!("{}:00000000", EXCHANGE_CODES[i % EXCHANGE_CODES.len()])
}

/// Pack one legacy v1 24-byte row: `u32 offset_ms LE + 5×f32 LE (open, high, low, close, volume)`.
fn pack_v1_row(offset_ms: u32, c: &ChartCandle) -> [u8; 24] {
    let mut out = [0u8; 24];
    out[0..4].copy_from_slice(&offset_ms.to_le_bytes());
    out[4..8].copy_from_slice(&c.open.to_le_bytes());
    out[8..12].copy_from_slice(&c.high.to_le_bytes());
    out[12..16].copy_from_slice(&c.low.to_le_bytes());
    out[16..20].copy_from_slice(&c.close.to_le_bytes());
    out[20..24].copy_from_slice(&c.volume.to_le_bytes());
    out
}

/// Generate one deterministic OHLCV series over `[from_ms, to_ms)` at `step_ms` spacing.
fn candle_series(
    rng: &mut Rng,
    from_ms: i64,
    to_ms: i64,
    step_ms: i64,
    base_price: f64,
) -> Vec<ChartCandle> {
    let mut out = Vec::new();
    let mut price = base_price;
    let mut t = from_ms;
    while t < to_ms {
        let open = price;
        let close = (open * (1.0 + rng.range(-0.01, 0.01))).max(0.0001);
        let high = open.max(close) * (1.0 + rng.range(0.0, 0.004));
        let low = open.min(close) * (1.0 - rng.range(0.0, 0.004));
        let volume = rng.range(1.0, 5000.0);
        out.push(ChartCandle {
            t_open_ms: t as f64,
            open: open as f32,
            high: high as f32,
            low: low as f32,
            close: close as f32,
            volume: volume as f32,
            quote_volume: (volume * (open + close) / 2.0) as f32,
        });
        price = close;
        t += step_ms;
    }
    out
}

/// Seed the requested disposable kline cache through the production write API.
fn main() {
    let mut args = std::env::args().skip(1);
    let data_dir = PathBuf::from(
        args.next()
            .expect("usage: gen_klines <data_dir> <span_days> [seed]"),
    );
    let span_days: i64 = args
        .next()
        .expect("span_days required")
        .parse()
        .expect("span_days must be an integer");
    let seed: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(20260820);

    assert!(
        moon_core::config::paths::set_data_dir_override(data_dir.clone()),
        "the data root must be installed before any path resolves"
    );
    let path = moon_core::config::paths::klines_db_path();
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }

    let end_ms = moon_core::util::now_unix_ms_i64();

    let cache = KlineCache::open(path.clone()).expect("open kline cache");
    let mut rng = Rng(seed);
    let mut written = 0usize;
    for i in 0..MARKETS {
        let market = market_name(i);
        let exchange = exchange_key(i);
        let base_price = rng.range(0.01, 70_000.0);
        for kind in KINDS {
            // `retention_days` prunes 1- and 5-minute chunks to 30 / 15 days on every open, so
            // covering the full requested span at those kinds would just synthesize rows the very
            // next reopen deletes — capped well inside each kind's own retention window instead.
            // The coarse kind's 3650-day retention has room for the whole span.
            let this_span_days = match kind {
                1 => span_days.min(20),
                5 => span_days.min(10),
                _ => span_days,
            };
            let from_ms = end_ms - this_span_days * DAY_MS;
            let rows = candle_series(&mut rng, from_ms, end_ms, kind as i64 * 60_000, base_price);
            written += rows.len();
            cache.merge_batch_blocking(vec![MergeItem {
                exchange: exchange.clone(),
                market: market.clone(),
                kind_min: kind,
                rows,
            }]);
        }
    }
    drop(cache);
    // The worker thread's Sender was just dropped, which ends its `rx.recv()` loop; the last
    // `merge_batch_blocking` above already waited for its own transaction to commit, so nothing
    // further needs to settle before a second connection can write.
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Direct legacy v1 rows for a handful of markets, one day each, so `read_rows`' v1/v2 merge
    // is exercised on read. `updated_ms` is older than the v2 rows above, matching the normal
    // (non-downgrade) case where v2 is the fresher write.
    let legacy_conn =
        rusqlite::Connection::open(&path).expect("open klines.sqlite for legacy rows");
    // Matches kind 60's own (uncapped) span, since the legacy row below is written under kind 60.
    let legacy_day = (end_ms - span_days * DAY_MS) / DAY_MS;
    let day_start = legacy_day * DAY_MS;
    let mut legacy_rows = 0usize;
    for i in 0..LEGACY_MARKETS {
        let market = market_name(i);
        let exchange = exchange_key(i);
        let base_price = rng.range(0.01, 70_000.0);
        let series = candle_series(
            &mut rng,
            day_start,
            day_start + DAY_MS,
            60 * 60_000,
            base_price,
        );
        let mut blob = Vec::new();
        for c in &series {
            let offset_ms = (c.t_open_ms as i64 - day_start) as u32;
            blob.extend_from_slice(&pack_v1_row(offset_ms, c));
        }
        legacy_rows += series.len();
        legacy_conn
            .execute(
                "INSERT OR REPLACE INTO chunks(exchange, market, kind, day, rows, updated_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    exchange,
                    market,
                    60u32,
                    legacy_day,
                    blob,
                    day_start - DAY_MS
                ],
            )
            .expect("insert legacy v1 chunk");
    }

    println!(
        "[OK] {}: {written} v2 rows across {MARKETS} markets x {} kinds, \
         {legacy_rows} legacy v1 rows across {LEGACY_MARKETS} markets",
        path.display(),
        KINDS.len()
    );
}
