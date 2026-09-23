//! The deal's own coin: its ranges over the core's windows, the last five seconds' move, and the
//! hour's pump and dump.
//!
//! The ranges (`d1m … d24h`) are the core's — see the module doc of [`super`]. The other three
//! are defined by the FAQ in words only and checked on 91 Binance trades (2026-09-23):
//!
//! - Pump1h (FAQ :1047 "the difference between the price an hour ago and the hour's high"):
//!   `(high / open − 1) · 100`, and Dump1h `(1 − low / open) · 100`, where "an hour ago" is the
//!   OPEN of the oldest of the core's candles in the hour's window — twelve closed and the open
//!   one (the core developer, 2026-09-23); off the price sixty minutes before the moment they had
//!   met the report exactly on 3 and 5 of 91 trades;
//! - d5s (no definition anywhere; RTTI `Last5sDelta`): the move over the last five-second
//!   bucket, `|last price / the last price a bucket earlier − 1| · 100`, median 0.07 pp — closer
//!   than the bucket's range (0.10 pp).
//!
//! The anchor on the report (see [`super::DeltaTrack`]) takes the offset out; what these carry
//! into the model is how they move.

use super::field::DeltaField;
use super::series::{Extremes, Series};
use super::{CANDLE_MS, LOOKBACK_MS, MINUTE_MS, STEP_MS};

/// How far back each window the coin's fields are read over reaches.
///
/// The short ones are whole five-second buckets, so their reach is their name.
///
/// The core keeps a candle while its CLOSE is younger than the window (the core developer,
/// 2026-09-23): d15m and d1h take three and twelve closed candles plus the one still open,
/// "d3h" and "d24h" closed candles only, four hours and twenty-five of them, and Pump1h / Dump1h
/// the hour's twelve plus the open one. The core closes its candles on the clock's five-minute
/// grid from its start and drifts off it (it closes one once more than five minutes passed,
/// checked about once a second), so the grid is only the model's guess at the phase. Measured
/// on 1 122 trades at the report's stamp (2026-09-23), the grid brings d3h's median error from
/// 0.034 to 0.022 pp, d24h's from 0.324 to 0.171, Pump1h's 0.313 → 0.302, Dump1h's 0.268 →
/// 0.250, and moves nothing downstream. d15m and d1h on the grid met the stamp closer too
/// (0.150 → 0.135 pp) but moved worse AFTER it — the window losing a candle at a grid close the
/// core had not reached: MoonHook takes, placed seconds past the stamp, went 38 → 35 within
/// 0.05 pp of the core's, the MoonShot corridor's level 0.213 → 0.221 % off the archive — so
/// those two keep a sliding window one candle past their name, counted from the moment.
const REACH: [i64; 7] = [
    MINUTE_MS,
    5 * MINUTE_MS,
    15 * MINUTE_MS + CANDLE_MS,
    60 * MINUTE_MS + CANDLE_MS,
    4 * 60 * MINUTE_MS,
    LOOKBACK_MS - CANDLE_MS,
    60 * MINUTE_MS,
];

/// Whether a window is counted from the last candle close ([`REACH`]).
const ON_CANDLES: [bool; 7] = [false, false, false, false, true, true, true];

/// Whether a window takes only closed candles — the one still open reaches "d3h" and "d24h"
/// only through d1h, which floors them.
const CLOSED_ONLY: [bool; 7] = [false, false, false, false, true, true, false];

/// The last candle close at or before a moment, on the clock's five-minute grid.
fn last_close(at: i64) -> i64 {
    at.div_euclid(CANDLE_MS) * CANDLE_MS
}
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
    /// The next item to enter the windows that read up to the moment.
    next: usize,
    /// The next item to enter the windows of closed candles ([`CLOSED_ONLY`]).
    next_closed: usize,
    /// The earliest item still inside the hour's window — its open is the open of the oldest
    /// candle there, the price Pump1h and Dump1h count from.
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
            next_closed: 0,
            hour_first: 0,
            prev: None,
        }
    }

    /// The coin's fields at a boundary, over what printed before it. Called at ascending
    /// boundaries; d5s answers only where the previous call was one step earlier.
    pub(super) fn at(&mut self, at: i64) -> CoinPoint {
        let series: &'a Series = self.series;
        let items = &series.items;
        let closed = last_close(at);
        while self.next < items.len() && items[self.next].end_ms <= at {
            for (w, window) in self.windows.iter_mut().enumerate() {
                if !CLOSED_ONLY[w] {
                    window.push(self.next, items);
                }
            }
            self.next += 1;
        }
        while self.next_closed < items.len() && items[self.next_closed].end_ms <= closed {
            for (w, window) in self.windows.iter_mut().enumerate() {
                if CLOSED_ONLY[w] {
                    window.push(self.next_closed, items);
                }
            }
            self.next_closed += 1;
        }
        // Each window's `(start, end]`: from the last close or the moment, back by its reach.
        let span = |w: usize| {
            let from = if ON_CANDLES[w] { closed } else { at } - REACH[w];
            let to = if CLOSED_ONLY[w] { closed } else { at };
            (from, to)
        };
        let ranges: [Option<f64>; 6] = std::array::from_fn(|w| {
            let (from, _) = span(w);
            self.windows[w].range_at(at, at - from, items)
        });
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
        // Pump and dump over the hour's candles, off the open of the oldest (the core developer,
        // 2026-09-23), not off the price sixty minutes before the moment.
        let (hour_start, _) = span(HOUR);
        while self.hour_first < self.next && items[self.hour_first].end_ms <= hour_start {
            self.hour_first += 1;
        }
        let (pump, dump) = match (
            self.windows[HOUR].extremes_at(at, at - hour_start, items),
            (self.hour_first < self.next).then(|| items[self.hour_first].open),
        ) {
            (Some((high, low)), Some(open)) if open > 0.0 => (
                Some(((high / open - 1.0) * 100.0).max(0.0)),
                Some(((open - low) / open * 100.0).max(0.0)),
            ),
            _ => (None, None),
        };
        let cover = |w: usize| {
            let (from, to) = span(w);
            series.covered_fraction(from, to)
        };
        [
            (ranges[M1], cover(M1)),
            (ranges[M5], cover(M5)),
            (ranges[M15], cover(M15)),
            (ranges[H1], cover(H1)),
            (long(ranges[H4]), cover(H4)),
            (long(ranges[H25]), cover(H25)),
            (d5s, series.covered_fraction(at - STEP_MS, at)),
            (pump, cover(HOUR)),
            (dump, cover(HOUR)),
        ]
    }
}
