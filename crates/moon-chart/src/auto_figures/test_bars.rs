//! Bars for the detector tests: synthetic series with a known answer, and the committed `chart-ace`
//! candles.

use std::path::PathBuf;

use moon_core::market::candles::{ChartCandle, estimate_quote_volume};

/// 2024-01-01 00:00 UTC, a Monday — a calendar fact, independent of the code under test.
pub const MONDAY_2024_01_01_MS: f64 = 1_704_067_200_000.0;
/// Five minutes, ms.
pub const M5_MS: i64 = 300_000;

/// Bar `i` of a 5-minute series starting at [`MONDAY_2024_01_01_MS`].
pub fn bar(i: usize, open: f32, high: f32, low: f32, close: f32) -> ChartCandle {
    ChartCandle {
        t_open_ms: MONDAY_2024_01_01_MS + (i as i64 * M5_MS) as f64,
        open,
        high,
        low,
        close,
        volume: 1.0,
        quote_volume: 1.0,
    }
}

/// A series from `(high, low)` pairs; each bar opens at its low and closes at its midpoint, so its
/// body never reaches its high.
pub fn from_hl(hl: &[(f32, f32)]) -> Vec<ChartCandle> {
    hl.iter()
        .enumerate()
        .map(|(i, &(h, l))| bar(i, l, h, l, (h + l) / 2.0))
        .collect()
}

/// The committed `chart-ace` candles of one stored kind (1 or 5 minutes), ascending.
///
/// Read straight from the packed rows, read-only. The cache's own reader cannot be used: opening a
/// `KlineCache` prunes chunks past their retention, and the fixture is dated, so it would read
/// nothing — and, pointed at the committed file, delete it. Rows are the legacy 24-byte layout
/// `u32 in-day offset ms + open, high, low, close, volume as f32`, the layout
/// `moon_core::market::kline_cache` documents for the `chunks` table. The turnover is what that
/// cache's own decoder makes of such a row on a 1- or 5-minute kind: `volume` is base, and
/// `quote_volume` the OHLC4 estimate (`estimate_quote_volume`) — so a detector reading
/// `quote_volume` here sees what it sees on the live cache.
pub fn ace(kind: u32) -> Vec<ChartCandle> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("moon-core")
        .join("fixtures")
        .join("chart-ace")
        .join("klines.sqlite");
    let conn =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open the committed chart-ace candles");
    let mut stmt = conn
        .prepare("SELECT day, rows FROM chunks WHERE kind = ?1 ORDER BY day")
        .expect("prepare");
    let chunks: Vec<(i64, Vec<u8>)> = stmt
        .query_map([kind], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    let mut out = Vec::new();
    for (day, blob) in chunks {
        for row in blob.chunks_exact(24) {
            let word = |i: usize| [row[i], row[i + 1], row[i + 2], row[i + 3]];
            let f = |i: usize| f32::from_le_bytes(word(i));
            let (open, high, low, close, volume) = (f(4), f(8), f(12), f(16), f(20));
            out.push(ChartCandle {
                t_open_ms: (day * 86_400_000 + i64::from(u32::from_le_bytes(word(0)))) as f64,
                open,
                high,
                low,
                close,
                volume,
                quote_volume: estimate_quote_volume(volume, open, high, low, close),
            });
        }
    }
    out.sort_by(|a, b| a.t_open_ms.total_cmp(&b.t_open_ms));
    out.dedup_by(|a, b| a.t_open_ms == b.t_open_ms);
    assert!(
        out.len() > 1_000,
        "the bench must carry a dense series, got {}",
        out.len()
    );
    out
}

/// The lowest low and highest high of a series — the range every level must lie inside.
pub fn range(bars: &[ChartCandle]) -> (f64, f64) {
    bars.iter().fold((f64::MAX, f64::MIN), |(lo, hi), b| {
        (lo.min(f64::from(b.low)), hi.max(f64::from(b.high)))
    })
}
