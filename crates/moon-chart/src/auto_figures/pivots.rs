//! Pivot highs and lows — the turning points `Swing`, `Trend` and `Squeeze` are built from.
//!
//! A pivot high at bar `i` has a high STRICTLY above each of the `left` bars before it and at least
//! as high as each of the `right` bars after it; a pivot low mirrors that with lows. The asymmetry
//! is deliberate: a plateau of equal highs yields exactly one pivot (its first bar) instead of one
//! per bar, which would otherwise count a single flat top as several touches.
//!
//! A pivot needs its `right` bars to exist, so the last `right` bars of the input can never be
//! pivots — the detector never looks ahead of the last closed bar. `left = right = 0` makes every
//! valid bar both a pivot high and a pivot low; that is the literal reading of "no neighbours
//! required", not an error.

use moon_core::market::candles::ChartCandle;

use super::bar_ok;

/// Which extreme of its bar a pivot is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PivotKind {
    High,
    Low,
}

/// One pivot: the bar's index in the input slice, its open time and the pivot price.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pivot {
    pub idx: usize,
    pub t_ms: f64,
    pub price: f64,
    pub kind: PivotKind,
}

/// Every pivot of `bars`, in bar order (a bar that is both a high and a low pivot lists the high
/// first), written into `out` so a caller that runs many detectors can reuse the buffer.
///
/// A window that contains an unreadable bar (see [`bar_ok`]) yields no pivot: an extreme measured
/// against a NaN neighbour is not an extreme. Fewer than `left + right + 1` bars yields none.
/// Cost O(n · (left + right)), at most O(510 · n).
pub fn find_pivots(bars: &[ChartCandle], left: u8, right: u8, out: &mut Vec<Pivot>) {
    out.clear();
    let (l, r) = (usize::from(left), usize::from(right));
    let n = bars.len();
    if n < l + r + 1 {
        return;
    }
    for i in l..n - r {
        let b = &bars[i];
        let neighbours = &bars[i - l..=i + r];
        if !neighbours.iter().all(bar_ok) {
            continue;
        }
        let before = &bars[i - l..i];
        let after = &bars[i + 1..=i + r];
        let is_high =
            before.iter().all(|x| x.high < b.high) && after.iter().all(|x| x.high <= b.high);
        let is_low = before.iter().all(|x| x.low > b.low) && after.iter().all(|x| x.low >= b.low);
        if is_high {
            out.push(Pivot {
                idx: i,
                t_ms: b.t_open_ms,
                price: f64::from(b.high),
                kind: PivotKind::High,
            });
        }
        if is_low {
            out.push(Pivot {
                idx: i,
                t_ms: b.t_open_ms,
                price: f64::from(b.low),
                kind: PivotKind::Low,
            });
        }
    }
}

#[cfg(test)]
mod tests;
