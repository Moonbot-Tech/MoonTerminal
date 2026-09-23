//! BTC's deltas, read off the BTC market of the deal's own exchange.
//!
//! What the core computes (FAQ :1068, :1073, :1074; moonproto `state/markets/prices.rs`):
//!
//! - `btc5mdelta`, `dbtc1m`: BTC's range over the last five minutes and the last minute,
//!   `(max / min − 1) · 100`;
//! - `btc1hdelta`: signed — how far BTC's price stands from its one-hour average, the average an
//!   exponential one stepped every thirty seconds with weight 0.01 (moonproto: `avg = p · 0.01 +
//!   avg · 0.99`), `(price − average) / average · 100`.
//!
//! The history is whatever bars the kline cache holds for that market — the minute bars where
//! something fetched them, the recorder's five-minute ones elsewhere. A window narrower than the
//! bar it is read off takes that bar whole, so on five-minute bars `dbtc1m` is a bar's range, not
//! a minute's: the summary says how much of each window the history covered.

use super::MINUTE_MS;
use super::field::DeltaField;
use super::series::{Extremes, Series};

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
    average: Option<f64>,
    /// The last complete bar's close, its end, and its length.
    last: Option<(f64, i64, i64)>,
}

impl<'a> BtcEval<'a> {
    pub(super) fn new(series: &'a Series) -> Self {
        Self {
            series,
            windows: [Extremes::new(), Extremes::new()],
            next: 0,
            average: None,
            last: None,
        }
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
            self.average = Some(match self.average {
                // Seeded as the core seeds it, off a candle's mean price.
                None => (item.open + item.high + item.low + item.close) / 4.0,
                Some(average) => {
                    let steps = (length / AVERAGE_STEP_MS).max(1) as i32;
                    let keep = AVERAGE_KEEP.powi(steps);
                    item.close * (1.0 - keep) + average * keep
                }
            });
            self.last = Some((item.close, item.end_ms, length));
            self.next += 1;
        }
        let fresh = self.last.filter(|&(_, end, _)| end > at - STALE_MS);
        let bar = fresh.map_or(0, |(_, _, length)| length);
        let reach_1m = MINUTE_MS.max(bar);
        let reach_5m = (5 * MINUTE_MS).max(bar);
        let btc1m = fresh.and_then(|_| self.windows[0].range_at(at, reach_1m, items));
        let btc5m = fresh.and_then(|_| self.windows[1].range_at(at, reach_5m, items));
        let btc1h = match (fresh, self.average) {
            (Some((price, _, _)), Some(average)) if average > 0.0 => {
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
