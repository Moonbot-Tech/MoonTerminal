//! Which rows of the core's 5-minute ring one kline-recorder pass keeps in the candle cache, and
//! in what form.
//!
//! The ring (moonproto `candles_5m`, 500 sealed candles ≈ 41.6 hours per market) is the only
//! history that covers the hours the CORE ran and the terminal did not: the recorder's own trade
//! aggregation starts when this process does, and measured on 19.11.2026 that left 7 087 of
//! 7 174 cached markets without a single complete UTC day. Read only into memory, the ring was
//! lost with every restart; kept in the cache it fills those holes for good — under its own kind,
//! `kline_cache::RING_5M_KIND`, which only the auto figures and the Screener's scan read (the
//! cache's module doc says why kind 5 must not hold it).
//!
//! # What is in the ring (moonproto, `state/history_store.rs` and `history_store/derived.rs`)
//!
//! Two writers push into it, and the ring keeps no mark of which one wrote a row:
//!
//! - the core's SNAPSHOT (`replace_candles_5m_from_snapshot`): the ring is cleared and the core's
//!   own sealed candles pushed in one batch. These are the rows the ring is kept for; their
//!   `volume` is quote money on every venue — MEASURED on 2026-11-25 against the turnover of the
//!   candles the terminal folded from trades, on codes 2, 3, 4, 6, 8, 9, 13, 15 and on the unknown
//!   221 and 222, whose DEEP rows are base. So the ring's unit is not the deep table's
//!   (`crate::venue::deep_volume_is_quote`), and it is not looked up per venue at all;
//! - the client's own LIVE seal (`seal_current_candle_if_due`), one row at a time, built from the
//!   futures trade stream: `volume` is `price * qty`, quote money on every venue. Such a row covers
//!   only time this terminal was connected and receiving those very trades — which the recorder
//!   already folds into real kind-5 candles in the same pass.
//!
//! The ring's sequence is what tells them apart: a clear resets it to zero, and each live seal adds
//! one. The recorder keeps the ring's cursor (`SeqRingReader::cursor_from_now`) from its last
//! pass; read from that cursor, a ring cleared and refilled since comes back CLIPPED (the cursor
//! lies past its end), while one that only gained live seals does not. So a pass takes the WHOLE
//! ring only when it holds a snapshot this recorder has not stored yet — the first pass for a
//! market, the first pass after its provider's client was replaced (another slot epoch: a
//! reconnect brings a fresh snapshot that covers the outage), a clipped cursor (a new snapshot
//! under the same client, or more pushes than the ring holds), or an oldest row older than last
//! time — and otherwise takes nothing: every row pushed since is a live seal, whose bucket the recorder's real candle already
//! holds. Skipping them is the cache's allowed economy (no ring row is needed where a real candle
//! exists).
//!
//! What this cannot see: the live seals pushed between a snapshot and the first pass that reads it
//! — none while the pass comes within five minutes of the snapshot (a live candle starts with the
//! first trade after the snapshot and seals at least five minutes later), a few after a longer
//! stall — and a new snapshot under the same client that holds at least as many rows as the old
//! ring had pushes and reaches no further back (the old cursor is then not clipped). Such a live
//! row is stored as a snapshot row, which is harmless: its volume is quote money like a snapshot
//! row's, and its bucket also holds the recorder's real candle, which every reader of the ring kind
//! prefers.
//!
//! The pass's mark moves only once the cache confirms the rows stored (`refresh.rs`): a failed
//! write leaves the previous mark, and the next pass takes the whole ring again.

use moonproto::state::SeqRingCursor;

use crate::market::candles::{ChartCandle, normalize_ohlc, split_wire_volume};
use crate::market::kline_cache::KIND_5M_MS;

/// What the recorder remembers about one market's ring between passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RingMark {
    /// The client slot's epoch the ring was read under; another epoch is another client.
    pub epoch: u64,
    /// The oldest usable stamp the ring held, Unix ms.
    pub oldest_ms: i64,
    /// The ring's end when it was read (`SeqRingReader::cursor_from_now`), taken before its rows.
    pub cursor: SeqRingCursor,
}

/// Whether a ring read under client `epoch`, whose oldest usable stamp is `oldest_ms`, holds a
/// snapshot not stored yet after `prev` (the module doc's four cases); `clipped` is whether a read
/// from `prev`'s cursor came back clipped.
pub(super) fn holds_new_snapshot(
    prev: Option<RingMark>,
    epoch: u64,
    oldest_ms: i64,
    clipped: bool,
) -> bool {
    match prev {
        None => true,
        Some(mark) => mark.epoch != epoch || clipped || oldest_ms < mark.oldest_ms,
    }
}

/// The open time a ring row stamped `stamp_ms` is stored at, or `None` when it must not be stored.
///
/// The stamp is the moment the row was SEALED, not a bucket boundary: moonproto stamps a sealed
/// candle with the time of the trade that sealed it (`seal_current_candle_if_due`, Delphi
/// `Recalc5mCandle`), which is at least five minutes after the candle's first trade. So
/// `stamp - 5 min` is the latest moment its period can have opened, and that moment floored to the
/// 5-minute grid is the bucket the row is filed under — for a market that trades, the bucket its
/// first trade fell in. Two rows sealed at least five minutes apart never share a bucket, and every
/// stored row lies on the grid the real kind-5 candles use, so a reader can match the two by
/// bucket.
///
/// `None` for moonproto's `MoonTime::ZERO` (the ring keeps such a row when the core's time was
/// unusable — stored, it would sit at 1970) and for a bucket not yet sealed on this machine's
/// clock, `cur_bucket_ms` being the open of the bucket still forming (a core clock running ahead
/// must not plant a bar in the future).
pub(super) fn ring_open_ms(stamp_ms: i64, cur_bucket_ms: i64) -> Option<i64> {
    if stamp_ms <= 0 {
        return None;
    }
    let open = (stamp_ms - KIND_5M_MS).div_euclid(KIND_5M_MS) * KIND_5M_MS;
    (open < cur_bucket_ms).then_some(open)
}

/// One ring row's wire fields, as moonproto's `Candle5mRow` carries them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct WireRow {
    /// The row's stamp, Unix ms — the moment it was sealed (see [`ring_open_ms`]).
    pub stamp_ms: i64,
    pub open: f32,
    pub high: f32,
    pub low: f32,
    pub close: f32,
    /// The wire volume figure of a SNAPSHOT row: quote money on every venue (module doc).
    pub volume: f32,
}

/// One snapshot row as the cache stores it under the ring kind: filed at [`ring_open_ms`], the
/// wire's prices normalised as every other wire row is (`normalize_ohlc`), and its volume split
/// into base and quote the way the chart's snapshot layer splits the same rows: the wire figure is
/// the quote, the base its OHLC4 estimate (`split_wire_volume` with `volume_is_quote = true`).
///
/// The open and close are stored as the wire gives them; no reader takes them for prices (the
/// ring kind's readers mark the whole row as having none).
///
/// `None` — nothing stored — for an unusable stamp, a wire price that is not a positive finite
/// number, and a volume that is negative or not finite: a cache row is kept for fifteen days, and a
/// wrong one would stand for all of them.
pub(super) fn ring_candle(row: WireRow, cur_bucket_ms: i64) -> Option<ChartCandle> {
    let t_open = ring_open_ms(row.stamp_ms, cur_bucket_ms)?;
    // All four wire prices first: `normalize_ohlc` spans a broken row's range over all four fields
    // and `f32::max` skips a NaN, so a NaN high would come out as the open, and a NaN open would
    // turn the base venue's quote estimate into a NaN.
    let price = |v: f32| v.is_finite() && v > 0.0;
    if ![row.open, row.high, row.low, row.close]
        .into_iter()
        .all(price)
    {
        return None;
    }
    let (open, high, low, close) = normalize_ohlc(row.open, row.high, row.low, row.close);
    if !(high.is_finite() && low.is_finite() && high > 0.0 && low > 0.0 && high >= low) {
        return None;
    }
    if !(row.volume.is_finite() && row.volume >= 0.0) {
        return None;
    }
    let (volume, quote_volume) = split_wire_volume(row.volume, open, high, low, close, true);
    Some(ChartCandle {
        t_open_ms: t_open as f64,
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
