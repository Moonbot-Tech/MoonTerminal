//! BTC's deltas, read off the BTC market of the deal's own exchange.
//!
//! What the core computes (FAQ :1068, :1073, :1074; moonproto `state/markets/prices.rs`):
//!
//! - `btc5mdelta`, `dbtc1m`: BTC's range over the last five minutes and the last minute,
//!   `(max / min − 1) · 100`;
//! - `btc1hdelta`: signed — how far BTC's price stands from its one-hour average,
//!   `(price − average) / average · 100`. The core developer (2026-09-23): the average is
//!   re-seeded on EVERY five-minute close as the mean OHLC4 of the last hour's closed candles,
//!   and between closes stepped every thirty seconds with weight 0.01 (moonproto: `avg = p ·
//!   0.01 + avg · 0.99`) — so it is an hour's mean with at most ten small steps on top, not the
//!   long exponential memory the steps alone would give. The closes are taken on the clock's
//!   five-minute grid, the phase the core starts on (see `coin::REACH`); a five-minute bar off
//!   the grid is a candle of its own. At the report's stamp (620 tracks, 2026-09-23) the re-seeded
//!   average brought `btc1hdelta`'s median error from 0.084 to 0.054 pp, 331 → 443 within 0.1.
//!
//! The history is whatever bars the kline cache holds for that market — the minute bars where
//! something fetched them, the recorder's five-minute ones elsewhere. A window narrower than the
//! bar it is read off takes that bar whole, so on five-minute bars `dbtc1m` is a bar's range, not
//! a minute's: the summary says how much of each window the history covered.

use super::field::DeltaField;
use super::series::{Extremes, Series};
use super::{CANDLE_MS, MINUTE_MS};

/// The step the core moves BTC's average by (moonproto: at most every 30 s).
const AVERAGE_STEP_MS: i64 = 30_000;

/// What the average keeps of itself at each step.
const AVERAGE_KEEP: f64 = 0.99;

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
    /// The seed and the close it was taken at.
    seed: Option<(i64, f64)>,
    /// The last complete bar's close, its end, and its length.
    last: Option<(f64, i64, i64)>,
}

impl<'a> BtcEval<'a> {
    pub(super) fn new(series: &'a Series) -> Self {
        Self {
            series,
            windows: [Extremes::new(), Extremes::new()],
            next: 0,
            hour_first: 0,
            seed: None,
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

    /// BTC's fields at a boundary, over the bars complete before it. Called at ascending
    /// boundaries.
    pub(super) fn at(&mut self, at: i64) -> BtcPoint {
        let series: &'a Series = self.series;
        let items = &series.items;
        while self.next < items.len() && items[self.next].end_ms <= at {
            let item = items[self.next];
            for window in &mut self.windows {
                window.push(self.next, items);
            }
            let length = (item.end_ms - item.from_ms).max(1);
            self.last = Some((item.close, item.end_ms, length));
            self.next += 1;
        }
        // Re-seeded on every five-minute close, then stepped toward the price every thirty
        // seconds since it.
        let close = at.div_euclid(CANDLE_MS) * CANDLE_MS;
        // An hour without a closed candle has no average: the field answers nothing there rather
        // than stepping an old seed toward the price for as long as the hole lasts.
        if self.seed.is_none_or(|(seeded, _)| seeded != close) {
            self.seed = self.hour_mean(close).map(|mean| (close, mean));
        }
        let fresh = self.last.filter(|&(_, end, _)| end > at - STALE_MS);
        let bar = fresh.map_or(0, |(_, _, length)| length);
        let reach_1m = MINUTE_MS.max(bar);
        let reach_5m = (5 * MINUTE_MS).max(bar);
        let btc1m = fresh.and_then(|_| self.windows[0].range_at(at, reach_1m, items));
        let btc5m = fresh.and_then(|_| self.windows[1].range_at(at, reach_5m, items));
        let btc1h = match (fresh, self.seed) {
            (Some((price, _, _)), Some((seeded, mean))) if mean > 0.0 => {
                let steps = ((at - seeded).max(0) / AVERAGE_STEP_MS) as i32;
                let keep = AVERAGE_KEEP.powi(steps);
                let average = price * (1.0 - keep) + mean * keep;
                Some((price - average) / average * 100.0)
            }
            _ => None,
        };
        let cover = |reach: i64| series.covered_fraction(at - reach, at);
        [
            (btc1m, cover(reach_1m)),
            (btc5m, cover(reach_5m)),
            (btc1h, cover(STALE_MS)),
        ]
    }
}
