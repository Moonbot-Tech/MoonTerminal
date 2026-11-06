//! Generated fixture adapter at the normal chart-history boundary.
//! It uses production candle aggregation and price fitting; GPU preparation remains unchanged.

use super::*;
use crate::fixture::dense::{CAPACITY, DenseHistory};

/// Camera bounds and invalidation supplied by the regular chart history caller.
pub(super) struct Viewport {
    pub window: (f32, f32),
    pub force_reset: bool,
    pub price_window: Option<(f32, f32)>,
}

/// Drain generated trades once per consumer, retaining an idle scene without new uploads.
pub(super) fn read(
    history: &DenseHistory,
    core: CoreId,
    epoch_ms: f64,
    viewport: Viewport,
    params: Option<&CandleReadParams>,
    cursor: &mut ChartHistoryCursor,
    out: &mut ChartHistoryBuffers,
) -> ChartHistoryRead {
    let Viewport {
        window,
        force_reset,
        price_window,
    } = viewport;
    let tf_ms = params.map_or(60_000, |params| params.tf_ms);
    let force_reset =
        force_reset || !cursor.candle_series.is_valid() || cursor.candle_series.tf_ms() != tf_ms;
    let reset = history.copy_since(
        if force_reset { None } else { cursor.dense_next },
        &mut cursor.trade_rows,
    );
    cursor.dense_next = Some(history.produced());
    rows_to_ticks(&cursor.trade_rows, &mut cursor.candle_ticks);
    let previous_revision = cursor.candle_series.revision();
    let dirty_from = cursor.candle_ticks.first().map(|tick| {
        let bucket = tick.time_ms.div_euclid(tf_ms as f64) * tf_ms as f64;
        cursor
            .candle_series
            .candles()
            .partition_point(|candle| candle.t_open_ms < bucket)
    });
    if reset || !cursor.trade_rows.is_empty() {
        // Only a few hundred candles are resampled; the dense retained ring is never rescanned.
        // Using its complete base also preserves candles whose source trades have been evicted.
        cursor
            .candle_series
            .rebuild(tf_ms, history.base_candles(), 60_000, &[]);
    }
    if reset {
        cursor
            .price_fit
            .replace_from(&cursor.trade_rows, true, i64::MIN);
    } else if !cursor.trade_rows.is_empty() {
        cursor.price_fit.append(&cursor.trade_rows);
        cursor.price_fit.cap_live(CAPACITY);
    }
    let mut read = ChartHistoryRead {
        provider: core,
        revision: history.produced(),
        combo_capacity: CAPACITY,
        combo_reset: reset,
        caught_up: true,
        last_price: Some(history.last_price()),
        candles_revision: cursor.candle_series.revision(),
        ..ChartHistoryRead::default()
    };
    if reset {
        read.replace_price_lines();
    }
    let trades_from = params.map_or(window.0, |params| params.trades_from_rel_ms);
    if trades_from.is_finite() {
        let from_rel = if trades_from == crate::market::candles::TRADES_FROM_UNBOUNDED {
            trades_from
        } else {
            trades_from.max(window.0)
        };
        let from = epoch_ms + f64::from(from_rel);
        let to = epoch_ms + f64::from(window.1);
        let limit = params.map_or(CAPACITY, |params| params.trades_limit);
        out.ticks.extend(
            cursor
                .candle_ticks
                .iter()
                .filter(|tick| tick.time_ms >= from && tick.time_ms < to)
                .copied(),
        );
        if out.ticks.len() > limit {
            out.ticks.drain(..out.ticks.len() - limit);
        }
        read.combo_left_rel_ms = out
            .ticks
            .first()
            .map(|tick| (tick.time_ms - epoch_ms) as f32);
    }
    if let Some((from, to)) = price_window {
        read.tick_price_range = cursor
            .price_fit
            .range(
                (epoch_ms + f64::from(from)) as i64,
                (epoch_ms + f64::from(to)) as i64,
            )
            .and_then(|(range, _)| range);
    }
    if let Some(params) = params {
        read.candles_changed = reset || read.candles_revision != params.shipped_revision;
        if read.candles_changed {
            read.candles_patch_from = (!reset && params.shipped_revision == previous_revision)
                .then_some(dirty_from)
                .flatten();
            out.candles.extend_from_slice(
                &cursor.candle_series.candles()[read.candles_patch_from.unwrap_or(0)..],
            );
        }
    }
    read
}

#[cfg(test)]
mod tests;
