//! Market source handle and retained-ring conversion helpers.

use super::*;

/// UI-agnostic market read-model bridge.
///
/// Feed threads publish only `SharedMoonClient` slots and lightweight wakes.
/// Consumers call this source when they are about to render: it pulls retained
/// MoonProto snapshot rows through per-consumer cursors into the shared
/// `MarketStore`, then exposes a read-only view by consumer core/market.
#[derive(Clone)]
pub struct MarketDataSource {
    pub(super) inner: Arc<RwLock<MarketDataSourceInner>>,
}

pub(super) fn moon_time_from_rel_ms(epoch_ms: f64, rel_ms: f32) -> MoonTime {
    MoonTime::from_unix_millis((epoch_ms + rel_ms as f64).round() as i64)
}

/// Drain a last-price or mark-price line through their shared control flow.
///
/// The branches differ only in cursor, buffer, output, and converter. A reset or first call places
/// the cursor at the first row at or after `to_time` — the edge of the window it copies — so the
/// next drain fills anything between that edge and now instead of leaving a gap when the pane is
/// panned into the past; subsequent calls drain new rows and accumulate `clipped` and `caught_up`
/// in `read`. A reset or a clipped drain copies the window `[from_time, to_time]` and answers
/// `Replace`; a drain that copied rows converts only those and answers `Append` — they may lie past
/// `to_time`, off-screen right, where the draw culls them. Call only when the reader exists.
#[allow(clippy::too_many_arguments)]
pub(super) fn drain_price_line<R: SeqRingTimedRow>(
    reader: &SeqRingReader<R>,
    from_time: MoonTime,
    to_time: MoonTime,
    force_reset: bool,
    cursor_slot: &mut Option<SeqRingCursor>,
    rows: &mut Vec<R>,
    out: &mut Vec<PricePoint>,
    read: &mut ChartHistoryRead,
    convert: impl Fn(&[R], &mut Vec<PricePoint>),
) -> PriceLineUpdate {
    read.price_line_capacity = read.price_line_capacity.max(reader.capacity());
    let reset = force_reset || cursor_slot.is_none();
    let mut update = PriceLineUpdate::None;
    if reset {
        *cursor_slot = Some(reader.cursor_at_or_after_time(to_time));
        update = PriceLineUpdate::Replace;
    } else if let Some(cur) = cursor_slot.as_mut() {
        // `drain_new_bounded` replaces `rows` with the drained batch.
        let meta = reader.drain_new_bounded(cur, reader.capacity(), rows);
        read.clipped |= meta.clipped;
        read.caught_up &= meta.caught_up;
        if meta.clipped {
            update = PriceLineUpdate::Replace;
        } else if meta.copied > 0 {
            convert(rows, out);
            update = PriceLineUpdate::Append;
        }
    }
    if update == PriceLineUpdate::Replace {
        // A clipped drain moved the cursor to the newest row, so its window runs through that row
        // or the next append would leave a gap; a reset parks the cursor at `to_time` instead.
        let copy_to = if reset {
            to_time
        } else {
            MoonTime::from_unix_millis(i64::MAX)
        };
        reader.copy_time_range(from_time, copy_to, reader.capacity(), rows);
        convert(rows, out);
    }
    if update != PriceLineUpdate::None {
        read.price_lines_changed = true;
    }
    update
}

pub(super) fn rows_to_ticks(rows: &[TradeHistoryRow], out: &mut Vec<Tick>) {
    out.clear();
    out.reserve(rows.len());
    out.extend(rows.iter().map(|r| Tick {
        time_ms: r.unix_millis() as f64,
        price: r.price,
        qty: r.quantity(),
        side: if r.is_buy() { Side::Buy } else { Side::Sell },
    }));
}

/// Convert last-price or mark-price line rows into chart points.
///
/// Both row types expose time through `SeqRingTimedRow` and price as the degenerate `(p, p)` range
/// through `SeqRingPriceRow`; these rows never return `None`.
pub(super) fn price_rows_to_points<R: SeqRingTimedRow + SeqRingPriceRow>(
    rows: &[R],
    out: &mut Vec<PricePoint>,
) {
    out.clear();
    out.reserve(rows.len());
    out.extend(rows.iter().filter_map(|p| {
        let (price, _) = p.seq_ring_price_range()?;
        Some(PricePoint {
            time_ms: p.seq_ring_time_ms() as f64,
            price,
        })
    }));
}

pub(super) fn trade_price_range(rows: &[TradeHistoryRow]) -> Option<(f32, f32)> {
    if rows.is_empty() {
        return None;
    }
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    for r in rows {
        lo = lo.min(r.price);
        hi = hi.max(r.price);
    }
    Some((lo, hi))
}

pub(super) fn cadence_phase_ms(provider: CoreId, market: &str, period_ms: u64) -> u64 {
    let mut sig = 0xcbf29ce484222325u64;
    sig ^= provider;
    sig = sig.wrapping_mul(0x100000001b3);
    for b in market.bytes() {
        sig ^= b as u64;
        sig = sig.wrapping_mul(0x100000001b3);
    }
    sig % period_ms.max(1)
}

pub(super) fn cadence_slot(elapsed_ms: u64, phase_ms: u64, period_ms: u64) -> Option<u64> {
    if elapsed_ms < phase_ms {
        None
    } else {
        Some((elapsed_ms - phase_ms) / period_ms.max(1))
    }
}
