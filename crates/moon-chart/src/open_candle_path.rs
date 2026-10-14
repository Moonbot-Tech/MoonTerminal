//! Composes the open candle's cached intraday prefix and retained five-second path.

use moon_core::market::candles::{ChartCandle, PATH_MINI_MS, PathMini};

/// One price range placed at its real time and duration on the plot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathBar {
    /// Open time relative to the chart epoch, in milliseconds.
    pub t_open_rel: f32,
    /// Width in milliseconds, including a clipped seam between data sources.
    pub duration_ms: f32,
    pub low: f32,
    pub high: f32,
    /// Whether the range's middle is not below the previous range's; colours the bar the way
    /// a rising candle is coloured.
    pub rising: bool,
}

/// Places mini-candles left of the raw dots, independently of the hide setting.
///
/// Args:
///     minis: Mini-candles, oldest first.
///     start_ms: Absolute opening of the current candle or an earlier hidden bucket.
///     dots_left_ms: Absolute time of the oldest raw dot; non-finite means no dots were placed.
///         An append of newly folded mini-candles passes `f64::INFINITY`: those prints already
///         left the ring, so they cannot sit under a dot.
///     epoch_ms: Chart epoch the output is relative to.
///     prev_mid: Middle of the last bar already placed, to colour the first new bar.
///     out: Receives the placed bars (appended).
///
/// Returns:
///     The middle of the last placed bar, or `prev_mid` when none was placed.
pub fn place_path(
    minis: &[PathMini],
    start_ms: f64,
    dots_left_ms: f64,
    epoch_ms: f64,
    prev_mid: Option<f32>,
    out: &mut Vec<PathBar>,
) -> Option<f32> {
    let mut prev = prev_mid;
    if !start_ms.is_finite() || dots_left_ms.is_nan() {
        return prev;
    }
    for mini in minis {
        if mini.t_open_ms < start_ms || mini.t_open_ms + PATH_MINI_MS > dots_left_ms {
            continue;
        }
        let mid = (mini.low + mini.high) * 0.5;
        out.push(PathBar {
            t_open_rel: (mini.t_open_ms - epoch_ms) as f32,
            duration_ms: PATH_MINI_MS as f32,
            low: mini.low,
            high: mini.high,
            rising: prev.is_none_or(|p| mid >= p),
        });
        prev = Some(mid);
    }
    prev
}

/// Places cached ranges up to the first mini-candle or raw dot. One-minute data wins every
/// overlap over five-minute fallback; partial seams clip time width rather than overlap.
/// `minutes == None` is unavailable, while `Some(&[])` is authoritative empty. Both may use
/// fallback without inventing prices. Timeframes at or below one minute have no cache prefix.
pub fn place_prefix(
    minutes: Option<&[ChartCandle]>,
    five: &[ChartCandle],
    window: (f64, f64),
    tf_ms: i64,
    epoch_ms: f64,
    out: &mut Vec<PathBar>,
) -> Option<f32> {
    let (start, end) = window;
    if tf_ms <= 60_000 || !start.is_finite() || !end.is_finite() || end <= start {
        return None;
    }
    let mut ranges = Vec::new();
    for row in minutes.unwrap_or_default() {
        if !row.t_open_ms.is_finite() {
            continue;
        }
        let left = row.t_open_ms.max(start);
        let right = (row.t_open_ms + 60_000.0).min(end);
        if left < right && valid_range(row.low, row.high) {
            ranges.push((left, right, row.low, row.high));
        }
    }
    ranges.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Keep the uncovered parts of a five-minute bucket even if only one minute is cached.
    let finer = ranges.clone();
    for row in five {
        if !row.t_open_ms.is_finite() || !valid_range(row.low, row.high) {
            continue;
        }
        let mut left = row.t_open_ms.max(start);
        let right = (row.t_open_ms + 300_000.0).min(end);
        for &(a, b, _, _) in &finer {
            if b <= left || a >= right {
                continue;
            }
            if a > left {
                ranges.push((left, a.min(right), row.low, row.high));
            }
            left = left.max(b);
        }
        if left < right {
            ranges.push((left, right, row.low, row.high));
        }
    }
    ranges.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut prev = None;
    for (left, right, low, high) in ranges {
        let mid = (low + high) * 0.5;
        out.push(PathBar {
            t_open_rel: (left - epoch_ms) as f32,
            duration_ms: (right - left) as f32,
            low,
            high,
            rising: prev.is_none_or(|p| mid >= p),
        });
        prev = Some(mid);
    }
    prev
}

/// Rejects malformed range data before it can flatten the chart's scale.
fn valid_range(low: f32, high: f32) -> bool {
    low.is_finite() && high.is_finite() && low > 0.0 && high >= low
}

#[cfg(test)]
mod tests;
