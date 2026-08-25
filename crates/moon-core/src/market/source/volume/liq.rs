//! What was LIQUIDATED over a period — the reference terminal's `L`.
//!
//! Beside the traded volume rather than inside it, because it is a different stream with different
//! reach. Liquidations arrive on their own retained ring, in the same row shape as trades, and the
//! chart already draws them as crosses. What they do NOT have is the second life trades get: a trade
//! evicted from its ring is compacted into a five-second mini-candle and keeps contributing for
//! another day, while an evicted liquidation is simply gone.
//!
//! So there is one source and one depth. A period reaching past the ring is reported incomplete —
//! the caption marks it — and no aggregate is invented to cover the difference. In practice the ring
//! is deep in TIME despite that: liquidations are rare compared to prints, so the same row count
//! spans much longer.
//!
//! Cost is the reason this needs no accumulator behind it: the rows are few, the window bounds the
//! copy at BOTH ends, and a market that liquidates nothing costs one bounds read.

use moonproto::state::{SeqRingReader, TradeHistoryRow};

use crate::session::CoreId;
use crate::util::time::now_unix_ms_i64;

use super::{MarketDataSource, VolumeAt, VolumeSpan};

/// What was liquidated over one span.
///
/// Both sides together, deliberately: a liquidation is a forced exit and the reading is "how much
/// got blown out here", not who was on which side of it. The row's own direction is still in the
/// ring for whenever that turns out to be worth a caption of its own.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LiqSpanReadout {
    /// Liquidated notional, in the market's quote currency.
    pub quote: f64,
    /// The same in the base coin. Always exact — every row carries its own quantity, since nothing
    /// aggregates these.
    pub base: f64,
    /// How many liquidations printed.
    pub count: u32,
    /// Whether the retained ring reached as far back as the span asked.
    pub complete: bool,
}

impl MarketDataSource {
    /// Return what was liquidated on one market over one span.
    ///
    /// Args:
    ///     core: Consumer core whose pane is being captioned; its PROVIDER owns the history.
    ///     market: Data-key market name.
    ///     span: Period to cover. A TRADE COUNT is not a period the liquidation ring answers — see
    ///         the return value.
    ///     at: Live edge, or a moment to centre the period on.
    ///
    /// Returns:
    ///     The figures, or `None` when the market has no liquidation ring, or when the span and
    ///     anchor do not describe a window at all.
    pub fn market_liq_span(
        &self,
        core: CoreId,
        market: &str,
        span: VolumeSpan,
        at: VolumeAt,
    ) -> Option<LiqSpanReadout> {
        let now = now_unix_ms_i64();
        let (from, to) = at.bounds(span, now)?;
        if to <= from {
            return None;
        }
        let provider = self.provider_of(core)?;
        // Cached on the same terms as the traded figures beside it: the pointer path asks once per
        // quantized moment, and a stack of panes on one coin must not each pay for the read.
        if let Some(hit) = self.liq_cached(provider, market, span, at, now) {
            return Some(hit);
        }
        let snapshot = self.core_client(provider)?.snapshot_versioned()?;
        let reader = snapshot.market_history_readers(market)?.liquidations?;
        let readout = read_range(&reader, from, to, now);
        self.liq_store(provider, market, span, at, now, readout);
        Some(readout)
    }
}

/// Sum the liquidations inside `[from, to)`.
fn read_range(
    reader: &SeqRingReader<TradeHistoryRow>,
    from: i64,
    to: i64,
    now: i64,
) -> LiqSpanReadout {
    let mut out = LiqSpanReadout::default();
    // Bounded at BOTH ends by SEQUENCE, not by time — see `super::copy_window`. A time-ranged copy
    // walks to the ring head testing every row past the far end, which on the pointer path is the
    // whole retained tail per mouse move.
    let mut rows = Vec::new();
    super::copy_window(reader, from, to, &mut rows);
    for row in &rows {
        let qty = f64::from(row.quantity());
        out.quote += f64::from(row.price) * qty;
        out.base += qty;
        out.count = out.count.saturating_add(1);
    }
    // Coverage is answered by the OLDEST row the ring still holds, not by what fell inside the
    // window: a quiet stretch inside a well-covered period is a real zero, while a period reaching
    // past the ring is a figure that is missing rows nobody can count.
    // An EMPTY ring is the common case, not a gap: most markets are never liquidated, and the ring
    // is allocated whether or not anything lands in it. Treating "no rows at all" as "not covered"
    // would print `~0` forever on every quiet coin — a permanent warning about nothing.
    //
    // The cost of that choice, stated plainly: a market subscribed seconds ago also has an empty
    // ring, and its zero is not yet a fact. The traded figures beside it carry their own mark for
    // exactly that stretch, so the block still says so.
    let reaches_back = match oldest_row_ms(reader) {
        Some(oldest) => oldest <= from,
        None => true,
    };
    // And the far end has to be in the PAST: a window centred on a point near the live edge reaches
    // into a stretch that has not happened, exactly as the traded figures beside it report.
    out.complete = reaches_back && to <= now;
    out
}

/// Timestamp of the oldest row the ring still holds.
///
/// One row read under the ring's own lock. The ring keeps its sequence numbers private outside the
/// protocol's diagnostics builds, and a sequence number is not a time anyway.
fn oldest_row_ms(reader: &SeqRingReader<TradeHistoryRow>) -> Option<i64> {
    reader.with_from_cursor(reader.cursor_from_oldest(), 1, |view| {
        let (first, second) = view.as_slices();
        first
            .first()
            .or_else(|| second.first())
            .map(|row| row.time.unix_millis())
    })
}

#[cfg(test)]
mod tests;
