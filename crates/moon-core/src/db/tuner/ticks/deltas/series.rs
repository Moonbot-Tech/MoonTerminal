//! What the deltas are read off: a market's history as bars and prints, the sliding extremes
//! of a window over it, and how much of a window the history covers.

use std::collections::VecDeque;

use super::Bar;
use crate::feed::types::Tick;

/// What printed over a stretch — a bar, or one print — as the windows take it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Item {
    /// Where the stretch began — a print's own moment.
    pub(super) from_ms: i64,
    /// Exclusive end: the item is complete, and joins a window, at a boundary not before it.
    pub(super) end_ms: i64,
    pub(super) open: f64,
    pub(super) high: f64,
    pub(super) low: f64,
    pub(super) close: f64,
}

/// A market's history, ascending by end: the bars that do not overlap the tape, then the prints.
pub(super) struct Series {
    pub(super) items: Vec<Item>,
    /// The stretches the history covers, merged, for [`Series::covered_fraction`].
    spans: Vec<(i64, i64)>,
}

impl Series {
    /// Build from bars and prints. A bar overlapping the tape's coverage is left out — the prints
    /// there are the truth, and a bar would carry prints from after the moment it is read at.
    pub(super) fn new(bars: &[Bar], ticks: &[Tick], covered: &[(i64, i64)]) -> Self {
        let overlaps_tape = |bar: &Bar| {
            covered
                .iter()
                .any(|&(from, to)| bar.from_ms < to && bar.to_ms > from)
        };
        let history: Vec<&Bar> = bars
            .iter()
            .filter(|b| {
                b.low > 0.0
                    && b.high >= b.low
                    && b.open > 0.0
                    && b.close > 0.0
                    && b.to_ms > b.from_ms
                    && !overlaps_tape(b)
            })
            .collect();
        let mut items: Vec<Item> = history
            .iter()
            .map(|b| Item {
                from_ms: b.from_ms,
                end_ms: b.to_ms,
                open: b.open,
                high: b.high,
                low: b.low,
                close: b.close,
            })
            .collect();
        items.extend(ticks.iter().filter_map(|t| {
            let price = f64::from(t.price);
            (price.is_finite() && price > 0.0).then_some(Item {
                from_ms: t.time_ms as i64,
                end_ms: t.time_ms as i64 + 1,
                open: price,
                high: price,
                low: price,
                close: price,
            })
        }));
        items.sort_by_key(|i| i.end_ms);
        let mut spans: Vec<(i64, i64)> = history
            .iter()
            .map(|b| (b.from_ms, b.to_ms))
            .chain(covered.iter().copied())
            .collect();
        spans.sort_unstable();
        let mut merged: Vec<(i64, i64)> = Vec::new();
        for (from, to) in spans {
            match merged.last_mut() {
                Some(last) if from <= last.1 => last.1 = last.1.max(to),
                _ => merged.push((from, to)),
            }
        }
        Self {
            items,
            spans: merged,
        }
    }

    /// The share of `[from, to)` the history covers, 0 … 1.
    pub(super) fn covered_fraction(&self, from: i64, to: i64) -> f64 {
        if to <= from {
            return 0.0;
        }
        let covered: i64 = self
            .spans
            .iter()
            .map(|&(a, b)| (b.min(to) - a.max(from)).max(0))
            .sum();
        covered as f64 / (to - from) as f64
    }
}

/// The sliding extremes of one window: two monotone queues of item indices, the highs
/// decreasing and the lows increasing from the front, so the front of each is the extreme of
/// what the window holds.
pub(super) struct Extremes {
    highs: VecDeque<usize>,
    lows: VecDeque<usize>,
}

impl Extremes {
    pub(super) fn new() -> Self {
        Self {
            highs: VecDeque::new(),
            lows: VecDeque::new(),
        }
    }

    pub(super) fn push(&mut self, index: usize, items: &[Item]) {
        let item = items[index];
        while self
            .highs
            .back()
            .is_some_and(|&i| items[i].high <= item.high)
        {
            self.highs.pop_back();
        }
        self.highs.push_back(index);
        while self.lows.back().is_some_and(|&i| items[i].low >= item.low) {
            self.lows.pop_back();
        }
        self.lows.push_back(index);
    }

    /// The window's `(high, low)` at a boundary: what ended inside `(at − reach, at]` — a bar
    /// that began before the window's start but ended inside it counts whole, as the core's
    /// candle does. `None` when the window holds nothing.
    pub(super) fn extremes_at(
        &mut self,
        at: i64,
        reach: i64,
        items: &[Item],
    ) -> Option<(f64, f64)> {
        let start = at - reach;
        while self
            .highs
            .front()
            .is_some_and(|&i| items[i].end_ms <= start)
        {
            self.highs.pop_front();
        }
        while self.lows.front().is_some_and(|&i| items[i].end_ms <= start) {
            self.lows.pop_front();
        }
        match (self.highs.front(), self.lows.front()) {
            (Some(&h), Some(&l)) => Some((items[h].high, items[l].low)),
            _ => None,
        }
    }

    /// The window's range at a boundary, per cent — `(max / min − 1) · 100` — or `None` when
    /// it holds nothing.
    pub(super) fn range_at(&mut self, at: i64, reach: i64, items: &[Item]) -> Option<f64> {
        let (high, low) = self.extremes_at(at, reach, items)?;
        (low > 0.0).then(|| (high / low - 1.0) * 100.0)
    }
}
