//! Windowed copies from the replay's prepared, time-ordered tick slice.

use crate::feed::Tick;

/// Copy only the requested slice and fit prices in the same pass, preserving the old filters.
pub(super) fn copy_ticks(
    ticks: &[Tick],
    bounds: (i64, i64),
    price_window: Option<(i64, i64)>,
    out: &mut Vec<Tick>,
) -> Option<(f32, f32)> {
    out.clear();
    // Replay builders publish ticks in time order. Integer comparisons deliberately preserve the
    // old inclusive millisecond clipping (a fractional timestamp is truncated, not rounded).
    let lo = ticks.partition_point(|tick| (tick.time_ms as i64) < bounds.0);
    let hi = ticks.partition_point(|tick| (tick.time_ms as i64) <= bounds.1);
    let selected = &ticks[lo..hi];
    out.reserve(selected.len());
    let mut range: Option<(f32, f32)> = None;
    for tick in selected {
        if !(tick.time_ms.is_finite() && tick.price.is_finite() && tick.price > 0.0) {
            continue;
        }
        out.push(*tick);
        if price_window
            .is_some_and(|(from, to)| tick.time_ms >= from as f64 && tick.time_ms <= to as f64)
        {
            range = Some(match range {
                None => (tick.price, tick.price),
                Some((lo, hi)) => (lo.min(tick.price), hi.max(tick.price)),
            });
        }
    }
    range
}

#[cfg(test)]
mod tests;
