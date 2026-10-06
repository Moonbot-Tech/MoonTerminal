//! Non-blocking cache-prefix polling and coarse-fill composition.

use super::*;

/// The one kline-cache prefix read a chart has in flight, with the key it was asked for.
pub(crate) struct PendingCacheRead {
    pub(super) read: crate::market::kline_cache::PendingPrefixRead,
    pub(super) exchange: String,
    pub(super) market: String,
    pub(super) kind: u32,
    pub(super) need_from: i64,
}

/// Keeps the chart's kline-cache prefix current without ever waiting on the cache worker.
///
/// At most one read is in flight; its reply is installed on the frame that finds it. The rows held
/// stay drawn until then, so a read in flight never shows as an empty history. A read that did not
/// happen is retried no more often than every `CACHE_RETRY_MS`, as a timed-out read was.
#[allow(clippy::too_many_arguments)] // every argument is a separate read-key part; a struct would only rename them
pub(crate) fn poll_cache_prefix(
    cursor: &mut ChartHistoryCursor,
    cache: Option<&crate::market::kline_cache::KlineCache>,
    exchange: Option<&str>,
    market: &str,
    native_kind_min: u32,
    need_from: i64,
    tf_ms: i64,
    now_unix_ms: i64,
) {
    use crate::market::kline_cache::{PrefixPoll, PrefixReadRequest};

    // Rows of another exchange or market are wrong, not merely old: drop them at once.
    if let Some(ex) = exchange {
        let same = cursor
            .cache_identity
            .as_ref()
            .is_some_and(|(e, m)| e == ex && m == market);
        if !same {
            cursor.cache_want_from = None;
            cursor.drop_cache_rows();
            cursor.invalidate_cache_prefix();
            cursor.cache_identity = Some((ex.to_string(), market.to_string()));
        }
    }
    // A read of another kind: its reply would be discarded anyway; waiting only delays the right one.
    if cursor
        .cache_pending
        .as_ref()
        .is_some_and(|p| p.kind != native_kind_min)
    {
        cursor.cache_pending = None;
    }

    if let Some(pending) = cursor.cache_pending.as_ref() {
        match pending.read.poll() {
            PrefixPoll::Pending => {}
            PrefixPoll::Lost => {
                cursor.cache_pending = None;
                cursor.cache_retry_at = Some(Instant::now());
            }
            PrefixPoll::Ready(rows) => {
                let pending = cursor
                    .cache_pending
                    .take()
                    .expect("pending read polled above");
                let current = exchange == Some(pending.exchange.as_str())
                    && market == pending.market
                    && native_kind_min == pending.kind;
                if current {
                    // Where the native kind leaves a hole, the worker also read the finer kinds:
                    // the 1-minute deep-history rows and the recorder's 5-minute rows, over the
                    // same window, aggregated to the native kind right here. Every supported
                    // timeframe is divisible by both. Reading them only when the native kind was
                    // EMPTY left a holey native kind to the range-only snapshot, which draws as
                    // bodies without wicks (#634); a native kind without holes has nothing to fill
                    // and costs no extra read.
                    let native_tf_ms = pending.kind as i64 * 60_000;
                    cursor.cache_rows_finer.clear();
                    if !rows.finer.is_empty() {
                        let parts: Vec<crate::market::candles::BasePart<'_>> = rows
                            .finer
                            .iter()
                            .map(|(fk, r)| crate::market::candles::BasePart {
                                rows: r,
                                tf_ms: *fk as i64 * 60_000,
                            })
                            .collect();
                        crate::market::candles::merge_bases(
                            native_tf_ms,
                            &parts,
                            &mut cursor.cache_rows_finer,
                        );
                    }
                    cursor.cache_rows = rows.native;
                    cursor.cache_rows_kind = pending.kind;
                    // Cache-only coarser layers extending the historical prefix. Kind-5 rows come
                    // from the recorder and possible deep-history writeback; the retained 5-minute
                    // snapshot is merged separately through `snap_part`.
                    cursor.cache_rows_5m = rows.m5;
                    cursor.cache_rows_1d = rows.d1;
                    cursor.cache_generation = cursor.cache_generation.wrapping_add(1);
                    cursor.cache_kind = Some(pending.kind);
                    cursor.cache_from_ms = pending.need_from;
                    cursor.cache_retry_at = None;
                    if !cursor.cache_rows.is_empty() || !cursor.cache_rows_finer.is_empty() {
                        log::log!(
                            super::SOURCE_TRACE_LEVEL,
                            "kline cache: префикс {market} kind{}: {} рядов, из finer kinds: {}",
                            cursor.cache_rows_kind,
                            cursor.cache_rows.len(),
                            cursor.cache_rows_finer.len()
                        );
                    }
                }
            }
        }
    }

    let stale = cursor.cache_kind != Some(native_kind_min) || need_from < cursor.cache_from_ms;
    if !stale {
        return;
    }
    let want_from = cursor
        .cache_want_from
        .map_or(need_from, |w| w.min(need_from));
    cursor.cache_want_from = Some(want_from);
    // A read that did not happen must not be remembered as a completed one; retry it, but no more
    // often than every `CACHE_RETRY_MS` so a busy worker is not asked again on every frame.
    let retry_due = cursor
        .cache_retry_at
        .is_none_or(|t| t.elapsed() >= Duration::from_millis(CACHE_RETRY_MS));
    if cursor.cache_pending.is_some() || !retry_due {
        return;
    }
    match (cache, exchange) {
        (Some(cache), Some(ex)) => {
            let req = PrefixReadRequest {
                exchange: ex.to_string(),
                market: market.to_string(),
                native_kind: native_kind_min,
                from_ms: want_from,
                now_ms: now_unix_ms,
                want_5m: tf_ms < 300_000,
                want_1d: tf_ms < 86_400_000,
                done: cursor.cache_done.clone(),
            };
            match cache.request_prefix(req) {
                Some(read) => {
                    cursor.cache_pending = Some(PendingCacheRead {
                        read,
                        exchange: ex.to_string(),
                        market: market.to_string(),
                        kind: native_kind_min,
                        need_from: want_from,
                    });
                    cursor.cache_want_from = None;
                    // The coarse filler layers are drawn at their own timeframe, so ones held over
                    // from a finer chart must not fill this one's gaps while the read is in flight.
                    let mut dropped = false;
                    if tf_ms >= 300_000 && !cursor.cache_rows_5m.is_empty() {
                        cursor.cache_rows_5m.clear();
                        dropped = true;
                    }
                    if tf_ms >= 86_400_000 && !cursor.cache_rows_1d.is_empty() {
                        cursor.cache_rows_1d.clear();
                        dropped = true;
                    }
                    if dropped {
                        cursor.cache_generation = cursor.cache_generation.wrapping_add(1);
                    }
                }
                None => cursor.cache_retry_at = Some(Instant::now()),
            }
        }
        _ => {
            // No cache at all: nothing to retry, and the window IS loaded — as empty.
            cursor.drop_cache_rows();
            cursor.cache_kind = Some(native_kind_min);
            cursor.cache_from_ms = need_from;
            cursor.cache_want_from = None;
        }
    }
}

/// Recomposes the chart's series with its cache-only coarser layers into `coarse_fill`.
pub(super) fn recompose_coarse_fill(cursor: &mut ChartHistoryCursor, series_tf_ms: i64) {
    let mut fill = std::mem::take(&mut cursor.coarse_fill);
    let mut layers: Vec<crate::market::candles::CoarseLayer<'_>> = Vec::new();
    // Order is PRIORITY: each layer's coverage is subtracted before the next is offered the
    // remainder. The local cache goes first because its rows are trade-derived with real OHLC; the
    // core's ring is range-only, so it fills what the cache could not — which after a restart is
    // most of the night.
    for (rows, tf) in [
        (&cursor.cache_rows_5m, 300_000.0f64),
        (&cursor.ring_rows_5m, 300_000.0f64),
        (&cursor.cache_rows_1d, 86_400_000.0f64),
    ] {
        // A layer finer than or equal to the series has nothing to add: those rows already reach
        // the series through `cache_part`/`snap_part` resampling, and re-adding them here would
        // draw every bucket twice.
        if (series_tf_ms as f64) >= tf {
            continue;
        }
        layers.push(crate::market::candles::CoarseLayer { rows, tf_ms: tf });
    }
    crate::market::candles::compose_with_coarse(
        cursor.candle_series.candles(),
        series_tf_ms as f64,
        &layers,
        &mut fill,
    );
    cursor.coarse_fill = fill;
    // A tail patch never changes which layers fed the fill, so only a recompose moves this bound.
    cursor.coarse_fill_max_tf = Some(
        layers
            .iter()
            // The fill stores each width as f32; bound by exactly what the filter reads.
            .map(|layer| f64::from(layer.tf_ms as f32))
            .fold(f64::from(series_tf_ms as f32), f64::max),
    );
}
