//! Series rebuild and deep-history writeback policy.

/// Left edge the candle series should be built from. Keeps the current floor while the needed
/// edge stays inside `[floor, floor + 4 * span]`; otherwise re-anchors one span LEFT of the needed
/// edge, so a leftward pan re-anchors at most once per span of travel and a rightward pan never
/// does until the retained excess exceeds four spans.
pub(crate) fn series_floor(current: i64, want_from_ms: i64, span_ms: i64) -> i64 {
    if current != i64::MAX
        && want_from_ms >= current
        && want_from_ms.saturating_sub(current) <= span_ms.saturating_mul(4)
    {
        current
    } else {
        want_from_ms.saturating_sub(span_ms)
    }
}

/// Every reason the candle series is rebuilt. A camera pan or a trade/combo reset is not one.
pub(crate) struct SeriesResetInputs {
    pub params_reset: bool,
    pub valid: bool,
    pub tf_changed: bool,
    pub trades_newly_available: bool,
    pub deep_sig_changed: bool,
    pub exchange_key_changed: bool,
    pub floor_moved: bool,
    pub cache_arrived: bool,
}

/// Whether the candle series must be rebuilt this read.
pub(crate) fn series_reset_due(i: &SeriesResetInputs) -> bool {
    i.params_reset
        || !i.valid
        || i.tf_changed
        || i.trades_newly_available
        || i.deep_sig_changed
        || i.exchange_key_changed
        || i.floor_moved
        || i.cache_arrived
}

/// Tail-only deep writebacks between two full ones.
pub(super) const DEEP_FULL_WRITEBACK_EVERY: u32 = 64;

/// Index of the first deep row still to write back. 0 = rewrite everything: nothing written yet
/// for this (exchange, kind), OLDER history arrived (the first row moved earlier), or fewer rows
/// than were written (a re-fetched response replaced rows). A ring that slid forward (first row
/// later, same or larger count) writes only the tail: the first row at or after the last written
/// bucket, which re-covers that bucket.
///
/// Accepted limit: a same-count correction of a middle row, or a merge the worker dropped, reaches
/// the cache only at the next full rewrite, at most `DEEP_FULL_WRITEBACK_EVERY` advances later.
pub(crate) fn deep_writeback_start(
    times: &[i64],
    same_key: bool,
    written_first: i64,
    written_last: i64,
    written_len: usize,
    since_full: u32,
) -> usize {
    if !same_key
        || since_full >= DEEP_FULL_WRITEBACK_EVERY
        || written_last == i64::MIN
        || times.first().is_none_or(|f| *f < written_first)
        || times.len() < written_len
    {
        0
    } else {
        times.partition_point(|t| *t < written_last)
    }
}
