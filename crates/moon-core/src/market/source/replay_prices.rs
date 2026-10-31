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

/// A stretch's edge and the ring point beside it closer than this are one line: the core sends
/// its list about every two seconds, so this is a few missed lists, not a stretch nobody recorded.
const GAP_JOIN_MS: f64 = 10_000.0;

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
///
/// A stretch is an island: where the point outside its edge lies further than [`GAP_JOIN_MS`]
/// away, a [`PricePoint::gap`] goes between them, so the line is not bridged straight across
/// ground the file holds nothing of — hours between two trades' recordings, or between a recording
/// and what the rings hold of this run.
pub fn overlay_stored_price_lines(
    window: (i64, i64),
    stored: &StoredPriceLines,
    out: &mut ChartHistoryBuffers,
) -> usize {
    let mut edges = Vec::with_capacity(stored.spans.len());
    for span in &stored.spans {
        let lo = span.from_ms.max(window.0);
        let hi = span.to_ms.min(window.1);
        if lo > hi {
            continue;
        }
        edges.push((lo as f64, hi as f64));
        for (points, recorded) in [
            (&mut out.last_points, &span.last),
            (&mut out.mark_points, &span.mark),
        ] {
            points.retain(|p| p.time_ms < lo as f64 || p.time_ms > hi as f64);
            clip_recorded(recorded, lo, hi, points);
        }
    }
    prepare_replay_price_lines(window, out);
    for points in [&mut out.last_points, &mut out.mark_points] {
        break_at_edges(&edges, points);
    }
    out.last_points.len().max(out.mark_points.len()).max(1)
}

/// Put a gap at each stretch edge of `points` whose neighbour outside lies past [`GAP_JOIN_MS`].
///
/// `points` are sorted and valid ([`prepare_replay_price_lines`]); each gap takes its edge's time,
/// so the line stays in time order.
fn break_at_edges(edges: &[(f64, f64)], points: &mut Vec<PricePoint>) {
    let mut breaks: Vec<(usize, f64)> = Vec::new();
    for &(lo, hi) in edges {
        let first = points.partition_point(|p| p.time_ms < lo);
        if first
            .checked_sub(1)
            .is_some_and(|before| lo - points[before].time_ms > GAP_JOIN_MS)
        {
            breaks.push((first, lo));
        }
        let past = points.partition_point(|p| p.time_ms <= hi);
        if points
            .get(past)
            .is_some_and(|after| after.time_ms - hi > GAP_JOIN_MS)
        {
            breaks.push((past, hi));
        }
    }
    if breaks.is_empty() {
        return;
    }
    breaks.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut merged = Vec::with_capacity(points.len() + breaks.len());
    let mut next = breaks.iter().peekable();
    for (i, point) in points.iter().enumerate() {
        while let Some(&(_, time_ms)) = next.next_if(|b| b.0 == i) {
            merged.push(PricePoint::gap(time_ms));
        }
        merged.push(*point);
    }
    merged.extend(next.map(|&(_, time_ms)| PricePoint::gap(time_ms)));
    *points = merged;
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
