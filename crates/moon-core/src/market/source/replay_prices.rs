//! Retained price lines for a trade replay, independent of live chart cursors.

use super::{ChartHistoryBuffers, MarketDataSource, price_rows_to_points};
use crate::feed::PricePoint;
use crate::session::CoreId;

impl MarketDataSource {
    /// Replace only the replay's price-line buffers from its provider's retained history.
    ///
    /// Bounds are inclusive UTC milliseconds and already clipped to the replay window. The
    /// shared archive gate requests missing history without blocking; an archive notification
    /// causes the chart to read again. Missing mark history remains empty, including on spot.
    /// Returns the capacity needed by either line, without changing any live consumer cursor.
    pub fn read_replay_price_lines(
        &self,
        core: CoreId,
        market: &str,
        window: (i64, i64),
        out: &mut ChartHistoryBuffers,
    ) -> usize {
        out.last_points.clear();
        out.mark_points.clear();
        let Some((provider, client, epoch, archive)) = ({
            let inner = self.inner.read().expect("market source poisoned");
            inner.core_provider.get(&core).and_then(|provider| {
                let (client, epoch) = inner.clients.get(provider)?.get_with_epoch()?;
                Some((*provider, client, epoch, inner.archive.clone()))
            })
        }) else {
            return 1;
        };
        let Some(readers) = client
            .snapshot_versioned()
            .and_then(|snapshot| snapshot.market_history_readers(market))
        else {
            return 1;
        };
        archive.request(provider, market, &client, epoch);
        if window.0 > window.1 {
            return 1;
        }
        if let Some(reader) = readers.last_prices {
            let mut rows = Vec::new();
            reader.copy_time_range_ms(
                window.0,
                window.1.saturating_add(1),
                reader.capacity(),
                &mut rows,
            );
            price_rows_to_points(&rows, &mut out.last_points);
        }
        if let Some(reader) = readers.mark_prices {
            let mut rows = Vec::new();
            reader.copy_time_range_ms(
                window.0,
                window.1.saturating_add(1),
                reader.capacity(),
                &mut rows,
            );
            price_rows_to_points(&rows, &mut out.mark_points);
        }
        prepare_replay_price_lines(window, out)
    }
}

/// Build drawable series from copied archive points, retaining no invalid or out-of-window row.
/// Sorting after the ring copy also handles retained resends without crossing time backwards.
fn prepare_replay_price_lines(window: (i64, i64), out: &mut ChartHistoryBuffers) -> usize {
    for points in [&mut out.last_points, &mut out.mark_points] {
        points.retain(|point: &PricePoint| {
            point.time_ms.is_finite()
                && point.price.is_finite()
                && point.price > 0.0
                && point.time_ms >= window.0 as f64
                && point.time_ms <= window.1 as f64
        });
        points.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
    }
    out.last_points.len().max(out.mark_points.len()).max(1)
}

#[cfg(test)]
mod tests;
