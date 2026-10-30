//! Retained price lines for a trade replay, independent of live chart cursors.

use super::{ChartHistoryBuffers, ChartHistoryCursor, MarketDataSource, price_rows_to_points};
use crate::feed::PricePoint;
use crate::market::price_recorder::StoredPriceLines;
use crate::session::CoreId;
use std::time::Instant;

/// Copied replay lines and the next local archive-demand retry, independent of live ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplayPriceLinesRead {
    /// Capacity required by either drawable line, including an empty line's minimum.
    pub capacity: usize,
    /// Local demand is still deferred; `None` means accepted or final for this client.
    pub retry_at: Option<Instant>,
}

impl MarketDataSource {
    /// Replace only the replay's price-line buffers from its provider's retained history.
    ///
    /// Bounds are inclusive UTC milliseconds and already clipped to the replay window. The
    /// shared archive gate requests missing history without blocking; an archive notification
    /// causes the chart to read again. Missing mark history remains empty, including on spot.
    /// Reuses this replay consumer's row scratch without advancing any live consumer cursor.
    /// Returns copied capacity and local demand's retry deadline, or `None` until readers exist.
    pub fn read_replay_price_lines(
        &self,
        core: CoreId,
        market: &str,
        window: (i64, i64),
        out: &mut ChartHistoryBuffers,
        cursor: &mut ChartHistoryCursor,
    ) -> Option<ReplayPriceLinesRead> {
        out.last_points.clear();
        out.mark_points.clear();
        let (provider, client, epoch, archive) = {
            let inner = self.inner.read().expect("market source poisoned");
            inner.core_provider.get(&core).and_then(|provider| {
                let (client, epoch) = inner.clients.get(provider)?.get_with_epoch()?;
                Some((*provider, client, epoch, inner.archive.clone()))
            })
        }?;
        let readers = client
            .snapshot_versioned()
            .and_then(|snapshot| snapshot.market_history_readers(market))?;
        archive.request(provider, market, &client, epoch);
        let retry_at = archive.retry_at(provider, market, epoch, Instant::now());
        if window.0 > window.1 {
            return Some(ReplayPriceLinesRead {
                capacity: 1,
                retry_at,
            });
        }
        if let Some(reader) = readers.last_prices {
            reader.copy_time_range_ms(
                window.0,
                window.1.saturating_add(1),
                reader.capacity(),
                &mut cursor.last_price_rows,
            );
            price_rows_to_points(&cursor.last_price_rows, &mut out.last_points);
        }
        if let Some(reader) = readers.mark_prices {
            reader.copy_time_range_ms(
                window.0,
                window.1.saturating_add(1),
                reader.capacity(),
                &mut cursor.mark_price_rows,
            );
            price_rows_to_points(&cursor.mark_price_rows, &mut out.mark_points);
        }
        Some(ReplayPriceLinesRead {
            capacity: prepare_replay_price_lines(window, out),
            retry_at,
        })
    }
}

/// Lay the price file's recorded lines over what the provider's rings gave a replay read.
///
/// Inside each recorded stretch the file is the whole truth and replaces the ring points there:
/// the rings start empty after every restart, so for a trade older than the run they hold nothing,
/// and where they do overlap the file holds the same list the rings were fed from. Outside every
/// stretch the ring points stay as they are. Each stretch is clipped to `window` and opens with the
/// value in force at its clipped start and closes with the value held to its clipped end, so a line
/// covers the stretch edge to edge rather than starting at its first change inside the window.
///
/// Bounds are inclusive UTC milliseconds, the same `window` the ring read took. Returns the
/// capacity either line needs, as [`MarketDataSource::read_replay_price_lines`] does.
pub fn overlay_stored_price_lines(
    window: (i64, i64),
    stored: &StoredPriceLines,
    out: &mut ChartHistoryBuffers,
) -> usize {
    for span in &stored.spans {
        let lo = span.from_ms.max(window.0);
        let hi = span.to_ms.min(window.1);
        if lo > hi {
            continue;
        }
        for (points, recorded) in [
            (&mut out.last_points, &span.last),
            (&mut out.mark_points, &span.mark),
        ] {
            points.retain(|p| p.time_ms < lo as f64 || p.time_ms > hi as f64);
            clip_recorded(recorded, lo, hi, points);
        }
    }
    prepare_replay_price_lines(window, out)
}

/// Append the part of one recorded line that speaks for `[lo, hi]`, held at both edges.
fn clip_recorded(recorded: &[PricePoint], lo: i64, hi: i64, out: &mut Vec<PricePoint>) {
    let (lo, hi) = (lo as f64, hi as f64);
    let start = recorded.partition_point(|p| p.time_ms <= lo);
    let mut held = start.checked_sub(1).map(|i| recorded[i]);
    if let Some(p) = held {
        out.push(PricePoint {
            time_ms: lo,
            price: p.price,
        });
    }
    for p in recorded[start..].iter().take_while(|p| p.time_ms <= hi) {
        out.push(*p);
        held = Some(*p);
    }
    if let Some(p) = held.filter(|p| p.time_ms < hi) {
        out.push(PricePoint {
            time_ms: hi,
            price: p.price,
        });
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
