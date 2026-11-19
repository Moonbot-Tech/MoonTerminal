//! Which rows of the core's 5-minute ring one kline-recorder pass keeps in the candle cache.
//!
//! The ring (moonproto `candles_5m`, 500 sealed candles ≈ 41.6 hours per market) is the only
//! history that covers the hours the CORE ran and the terminal did not: the recorder's own trade
//! aggregation starts when this process does, and measured on 19.11.2026 that left 7 087 of
//! 7 174 cached markets without a single complete UTC day. Read only into memory, the ring was
//! lost with every restart; kept in the cache (`KlineCache::fill_range_batch_blocking`), it fills
//! the holes for good — where no real candle exists, since a ring row carries only the range.
//!
//! One pass stores either the WHOLE ring or only the rows sealed since the previous pass:
//!
//! - the whole ring on the first pass for a market, on the first pass after its provider's client
//!   was replaced (a reconnect brings a fresh snapshot that may cover the outage), and when the
//!   ring's oldest row is OLDER than the previous pass saw — a snapshot that landed after the
//!   client had already sealed a few live rows, which a "newer than last time" cursor would skip;
//! - otherwise only the rows stamped after the newest one stored, normally one per five minutes.
//!
//! Re-offering a row already stored writes nothing (the cache compares it), so the whole-ring
//! case is safe to repeat; the cursor exists to keep the steady state from re-reading and
//! re-comparing 500 rows per market every minute.

use crate::market::candles::{ChartCandle, normalize_ohlc, split_wire_volume};

/// The ring's period, ms. Its rows are stamped at the period's END, so a row's open is one of
/// these before its stamp.
pub(super) const RING_TF_MS: i64 = 300_000;

/// What the recorder remembers about one market's ring between passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RingMark {
    /// The client slot's epoch the ring was read under; another epoch is another client.
    pub epoch: u64,
    /// The oldest usable stamp the ring held, Unix ms.
    pub oldest_ms: i64,
    /// The newest usable stamp offered to the cache, Unix ms.
    pub newest_ms: i64,
}

/// Which of the ring's rows a pass offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RingPlan {
    /// Every usable row.
    Whole,
    /// The rows stamped strictly after this moment, Unix ms.
    After(i64),
}

impl RingPlan {
    /// Whether a row stamped `stamp_ms` is offered under this plan.
    pub(super) fn takes(self, stamp_ms: i64) -> bool {
        match self {
            RingPlan::Whole => true,
            RingPlan::After(after) => stamp_ms > after,
        }
    }
}

/// The plan for a ring whose oldest usable stamp is `oldest_ms`, read under client `epoch`, after
/// `prev` (see the module doc for the three cases that take the whole ring).
pub(super) fn plan(prev: Option<RingMark>, epoch: u64, oldest_ms: i64) -> RingPlan {
    match prev {
        Some(mark) if mark.epoch == epoch && oldest_ms >= mark.oldest_ms => {
            RingPlan::After(mark.newest_ms)
        }
        _ => RingPlan::Whole,
    }
}

/// Whether a ring stamp is one a pass may store: not moonproto's `MoonTime::ZERO` (the ring keeps
/// such a row when the core's time was unusable — stored, it would sit at 1970) and sealed by
/// `cur_bucket_ms`, the open of the bucket still forming on this machine's clock (a core clock
/// running ahead must not plant a bar in the future, which a later real candle could not displace
/// in time to matter).
pub(super) fn usable_stamp(stamp_ms: i64, cur_bucket_ms: i64) -> bool {
    stamp_ms > 0 && stamp_ms - RING_TF_MS < cur_bucket_ms
}

/// One ring row's wire fields, as moonproto's `Candle5mRow` carries them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct WireRow {
    /// The row's stamp, Unix ms — the END of its period.
    pub stamp_ms: i64,
    pub open: f32,
    pub high: f32,
    pub low: f32,
    pub close: f32,
    /// The wire volume figure: base coins, or quote money on a venue that sends turnover.
    pub volume: f32,
}

/// One ring row as the cache stores it: opened one period before its stamp, the wire's prices
/// normalised as every other wire row is (`normalize_ohlc` — a ring row's `open == high`,
/// `close == low` passes unchanged and keeps its range-only form), and its volume split into base
/// and quote the way the chart's snapshot layer splits it (`split_wire_volume` with the venue's
/// `volume_is_quote`). `None` for an unusable stamp ([`usable_stamp`]) or a price that is not
/// positive and finite.
pub(super) fn ring_candle(
    row: WireRow,
    volume_is_quote: bool,
    cur_bucket_ms: i64,
) -> Option<ChartCandle> {
    if !usable_stamp(row.stamp_ms, cur_bucket_ms) {
        return None;
    }
    let (open, high, low, close) = normalize_ohlc(row.open, row.high, row.low, row.close);
    if !(high.is_finite() && low.is_finite() && high > 0.0 && low > 0.0) {
        return None;
    }
    let (volume, quote_volume) =
        split_wire_volume(row.volume, open, high, low, close, volume_is_quote);
    Some(ChartCandle {
        t_open_ms: (row.stamp_ms - RING_TF_MS) as f64,
        open,
        high,
        low,
        close,
        volume,
        quote_volume,
    })
}

#[cfg(test)]
mod tests;
