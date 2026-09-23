//! The deal's own coin: its ranges over the core's windows, the last five seconds' move, and the
//! hour's pump and dump.
//!
//! The ranges (`d1m … d24h`) are the core's — see the module doc of [`super`]. The other three
//! are defined by the FAQ in words only and checked on 91 Binance trades (2026-09-23):
//!
//! - Pump1h (FAQ :1047 "the difference between the price an hour ago and the hour's high"):
//!   `(high / price an hour ago − 1) · 100` met the report exactly on 3 trades, median error
//!   0.24 pp — the core's "an hour ago" is a moment of its own;
//! - Dump1h ("… and the hour's low"): `(price an hour ago − low) / price an hour ago · 100`,
//!   exactly on 5, median 0.15 pp;
//! - d5s (no definition anywhere; RTTI `Last5sDelta`): the move over the last five-second
//!   bucket, `|last price / the last price a bucket earlier − 1| · 100`, median 0.07 pp — closer
//!   than the bucket's range (0.10 pp).
//!
//! The anchor on the report (see [`super::DeltaTrack`]) takes the offset out; what these carry
//! into the model is how they move.

use super::field::DeltaField;
use super::series::{Extremes, Series};
use super::{CANDLE_MS, LOOKBACK_MS, MINUTE_MS, STEP_MS};

/// How far back each window the coin's fields are read over reaches: the short ones are whole
/// five-second buckets, so their reach is their name; a window over the core's candles reaches
/// one candle further (measured on 86 trades, d15m's median error fell from 0.21 pp to 0.02 pp
/// with the extra candle). The last one is the plain hour Pump1h and Dump1h look back over.
const REACH: [i64; 7] = [
    MINUTE_MS,
    5 * MINUTE_MS,
    15 * MINUTE_MS + CANDLE_MS,
    60 * MINUTE_MS + CANDLE_MS,
    4 * 60 * MINUTE_MS + CANDLE_MS,
    LOOKBACK_MS,
    60 * MINUTE_MS,
];
const M1: usize = 0;
const M5: usize = 1;
const M15: usize = 2;
const H1: usize = 3;
const H4: usize = 4;
const H25: usize = 5;
const HOUR: usize = 6;

/// The coin's fields at one boundary: the value, `None` when the window holds nothing, and the
/// share of the window the history covers.
pub(super) type CoinPoint = [(Option<f64>, f64); 9];

/// The coin's fields, in the order [`CoinPoint`] carries them.
pub(super) const FIELDS: [DeltaField; 9] = [
    DeltaField::D1m,
    DeltaField::D5m,
    DeltaField::D15m,
    DeltaField::D1h,
    DeltaField::D3h,
    DeltaField::D24h,
    DeltaField::D5s,
    DeltaField::Pump1h,
    DeltaField::Dump1h,
];

/// Walks the coin's history boundary by boundary, ascending.
pub(super) struct CoinEval<'a> {
    series: &'a Series,
    windows: [Extremes; 7],
    /// The next item to enter the windows.
    next: usize,
    /// The earliest item still inside the plain hour — its open is "the price an hour ago".
    hour_first: usize,
    /// The last price before the previous boundary, and that boundary.
    prev: Option<(i64, f64)>,
}

impl<'a> CoinEval<'a> {
    pub(super) fn new(series: &'a Series) -> Self {
        Self {
            series,
            windows: std::array::from_fn(|_| Extremes::new()),
            next: 0,
            hour_first: 0,
            prev: None,
        }
    }

    /// The coin's fields at a boundary, over what printed before it. Called at ascending
    /// boundaries; d5s answers only where the previous call was one step earlier.
    pub(super) fn at(&mut self, at: i64) -> CoinPoint {
        let series: &'a Series = self.series;
        let items = &series.items;
        while self.next < items.len() && items[self.next].end_ms <= at {
            for window in &mut self.windows {
                window.push(self.next, items);
            }
            self.next += 1;
        }
        let ranges: [Option<f64>; 6] =
            std::array::from_fn(|w| self.windows[w].range_at(at, REACH[w], items));
        let long = |wide: Option<f64>| match (ranges[H1], wide) {
            (Some(h1), Some(w)) => Some(h1.max(w)),
            (h1, w) => h1.or(w),
        };
        // The last price before this boundary against the one before the previous.
        let last = self.next.checked_sub(1).map(|i| items[i].close);
        let d5s = match (self.prev, last) {
            (Some((was, before)), Some(now)) if was == at - STEP_MS && before > 0.0 => {
                Some((now / before - 1.0).abs() * 100.0)
            }
            _ => None,
        };
        if let Some(now) = last {
            self.prev = Some((at, now));
        }
        // Pump and dump over the plain hour, off the price it began at.
        let hour_start = at - REACH[HOUR];
        while self.hour_first < self.next && items[self.hour_first].end_ms <= hour_start {
            self.hour_first += 1;
        }
        let (pump, dump) = match (
            self.windows[HOUR].extremes_at(at, REACH[HOUR], items),
            (self.hour_first < self.next).then(|| items[self.hour_first].open),
        ) {
            (Some((high, low)), Some(open)) if open > 0.0 => (
                Some(((high / open - 1.0) * 100.0).max(0.0)),
                Some(((open - low) / open * 100.0).max(0.0)),
            ),
            _ => (None, None),
        };
        let cover = |reach: i64| series.covered_fraction(at - reach, at);
        [
            (ranges[M1], cover(REACH[M1])),
            (ranges[M5], cover(REACH[M5])),
            (ranges[M15], cover(REACH[M15])),
            (ranges[H1], cover(REACH[H1])),
            (long(ranges[H4]), cover(REACH[H4])),
            (long(ranges[H25]), cover(REACH[H25])),
            (d5s, cover(STEP_MS)),
            (pump, cover(REACH[HOUR])),
            (dump, cover(REACH[HOUR])),
        ]
    }
}
