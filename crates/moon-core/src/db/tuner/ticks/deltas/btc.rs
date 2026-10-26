//! BTC's deltas, read off the BTC market of the deal's own exchange.
//!
//! BTC1m and BTC5m are ranges over the last minute and five minutes. BTC1h is signed:
//! `(price - average) / average * 100`. The average is re-seeded at every five-minute close
//! from the hour's mean OHLC4, then stepped once on the first update strictly more than thirty
//! seconds after the previous step. Each step uses that update's own price; missed steps are
//! never caught up. The core uses mid (Bid + Ask) / 2; trade closes are the nearest observable
//! price in the history.
//! Source: https://moonbot.pro/documentation/filters-tab/filters-delta-tab
//!
//! History uses minute bars where available and five-minute bars elsewhere. A one-minute field
//! over five-minute bars is left unanswered, with zero coverage, and falls back to the report
//! snapshot rather than attributing a five-minute range to the last minute.

use super::field::DeltaField;
use super::series::{Extremes, Series};
use super::{CANDLE_MS, MINUTE_MS};

/// An update must arrive strictly after this interval to move BTC's average.
const AVERAGE_STEP_MS: i64 = 30_000;

/// What the average keeps of itself at each step.
const AVERAGE_KEEP: f64 = 0.99;

/// Move the average once toward the price observed at this step, preserving earlier steps.
pub(super) fn btc_avg_step(avg: f64, price: f64) -> f64 {
    AVERAGE_KEEP * avg + 0.01 * price
}

/// The oldest BTC print the average may stand on, relative to the boundary: past it the bars
/// have stopped and the price is stale.
const STALE_MS: i64 = 60 * MINUTE_MS;

/// BTC's fields, in the order [`BtcPoint`] carries them.
pub(super) const FIELDS: [DeltaField; 3] =
    [DeltaField::Btc1m, DeltaField::Btc5m, DeltaField::Btc1h];

/// BTC's fields at one boundary: the value, `None` when the history has nothing for it, and the
/// share of the window the history covers.
pub(super) type BtcPoint = [(Option<f64>, f64); 3];

/// Walks BTC's bars boundary by boundary, ascending.
pub(super) struct BtcEval<'a> {
    series: &'a Series,
    windows: [Extremes; 2],
    next: usize,
    /// The first item whose candle is still inside the hour the average is seeded from.
    hour_first: usize,
    /// The first item still inside the one-minute range, including a possible coarse bar.
    minute_first: usize,
    /// Coarse items inside `[minute_first, next)` make the minute's range unanswerable.
    minute_coarse: usize,
    /// The close at which the current hour mean was seeded.
    seeded_ms: Option<i64>,
    /// The running average, stepped using each update's own close.
    avg: Option<f64>,
    /// The next update must end strictly after this moment to step the average.
    next_step_ms: i64,
    /// The last complete bar's close, its end, and its length.
    last: Option<(f64, i64, i64)>,
}

impl<'a> BtcEval<'a> {
    /// Start an ascending walk without a seed or a last observed price.
    pub(super) fn new(series: &'a Series) -> Self {
        Self {
            series,
            windows: [Extremes::new(), Extremes::new()],
            next: 0,
            hour_first: 0,
            minute_first: 0,
            minute_coarse: 0,
            seeded_ms: None,
            avg: None,
            next_step_ms: 0,
            last: None,
        }
    }

    /// The mean OHLC4 of the candles that closed in the hour up to `close`, the core's five-minute
    /// candles put together from whatever bars end inside each: its open the first bar's, its
    /// close the last's. `None` when the hour holds none.
    fn hour_mean(&mut self, close: i64) -> Option<f64> {
        let series: &'a Series = self.series;
        let items = &series.items;
        let from = close - 60 * MINUTE_MS;
        while self.hour_first < items.len() && items[self.hour_first].end_ms <= from {
            self.hour_first += 1;
        }
        let (mut sum, mut count) = (0.0, 0usize);
        // The candle being put together: its close on the grid, open, high, low, close.
        let mut candle: Option<(i64, f64, f64, f64, f64)> = None;
        let mut flush = |c: Option<(i64, f64, f64, f64, f64)>| {
            if let Some((_, o, h, l, c)) = c {
                sum += (o + h + l + c) / 4.0;
                count += 1;
            }
        };
        for item in items[self.hour_first..]
            .iter()
            .take_while(|i| i.end_ms <= close)
        {
            // The grid close of the candle the bar ends in: `end` inside `(c − 5 min, c]`.
            let grid = (item.end_ms + CANDLE_MS - 1).div_euclid(CANDLE_MS) * CANDLE_MS;
            candle = match candle {
                Some((g, o, h, l, _)) if g == grid => {
                    Some((g, o, h.max(item.high), l.min(item.low), item.close))
                }
                other => {
                    flush(other);
                    Some((grid, item.open, item.high, item.low, item.close))
                }
            };
        }
        flush(candle);
        (count > 0).then(|| sum / count as f64)
    }

    /// Reset the average at a new grid close, including when the hour contains no candles.
    fn reseed(&mut self, at: i64) {
        let close = at.div_euclid(CANDLE_MS) * CANDLE_MS;
        if self.seeded_ms != Some(close) {
            self.avg = self.hour_mean(close);
            self.seeded_ms = Some(close);
            self.next_step_ms = close + AVERAGE_STEP_MS;
        }
    }

    /// BTC's fields at a boundary, over the bars complete before it. Called at ascending
    /// boundaries.
    pub(super) fn at(&mut self, at: i64) -> BtcPoint {
        let series: &'a Series = self.series;
        let items = &series.items;
        while self.next < items.len() && items[self.next].end_ms <= at {
            let item = items[self.next];
            self.reseed(item.end_ms);
            if item.end_ms > self.next_step_ms {
                self.avg = self.avg.map(|avg| btc_avg_step(avg, item.close));
                self.next_step_ms = item.end_ms + AVERAGE_STEP_MS;
            }
            for window in &mut self.windows {
                window.push(self.next, items);
            }
            let length = (item.end_ms - item.from_ms).max(1);
            self.minute_coarse += usize::from(length > MINUTE_MS);
            self.last = Some((item.close, item.end_ms, length));
            self.next += 1;
        }
        // An hour without a closed candle has no average: the field answers nothing there rather
        // than stepping an old seed toward the price for as long as the hole lasts.
        self.reseed(at);
        while self.minute_first < self.next && items[self.minute_first].end_ms <= at - MINUTE_MS {
            let item = items[self.minute_first];
            self.minute_coarse -= usize::from(item.end_ms - item.from_ms > MINUTE_MS);
            self.minute_first += 1;
        }
        let fresh = self.last.filter(|&(_, end, _)| end > at - STALE_MS);
        let bar = fresh.map_or(0, |(_, _, length)| length);
        let reach_1m = MINUTE_MS;
        let reach_5m = (5 * MINUTE_MS).max(bar);
        let coarse_1m = self.minute_coarse > 0;
        let btc1m = fresh
            .filter(|_| !coarse_1m)
            .and_then(|_| self.windows[0].range_at(at, reach_1m, items));
        let btc5m = fresh.and_then(|_| self.windows[1].range_at(at, reach_5m, items));
        let btc1h = match (fresh, self.avg) {
            (Some((price, _, _)), Some(average)) if average > 0.0 => {
                Some((price - average) / average * 100.0)
            }
            _ => None,
        };
        let cover = |reach: i64| series.covered_fraction(at - reach, at);
        [
            (btc1m, if coarse_1m { 0.0 } else { cover(reach_1m) }),
            (btc5m, cover(reach_5m)),
            (btc1h, cover(STALE_MS)),
        ]
    }
}
