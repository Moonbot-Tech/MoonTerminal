//! The deal's own coin: its ranges over the core's windows, the smoothed five-second range, and the
//! hour's pump and dump.
//!
//! The ranges (`d1m ... d24h`) follow the windows in [`super`]. Pump1h and Dump1h use the
//! hour's high and low relative to the open of the oldest candle in that hour's window.
//!
//! Last5sDelta reads the range of prints over 5100 ms plus the border print at a 250 ms cadence.
//! It rises instantly, smooths a lower range with weights 0.8/0.2, and decays by 0.98 in an
//! empty slot. The core samples its average price about every 250 ms; raw prints are the nearest
//! observable input here.
//! Source: https://moonbot.pro/documentation/filters-tab/filters-delta-tab
//!
//! The anchor on the report (see [`super::DeltaTrack`]) takes the offset out; what these carry
//! into the model is how they move.

use std::collections::VecDeque;

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

/// Sampling cadence for complete raw prints in Last5sDelta.
const SLOT_MS: i64 = 250;
/// Empty cadence slots retain this fraction of the previous short-range value.
const QUIET_KEEP: f64 = 0.98;
/// Short-range lookback before retaining the border print.
const WINDOW_MS: i64 = 5_100;

/// Apply an instant rise, a smoothed fall, or an empty-slot decay to Last5sDelta.
pub(super) fn last5s_step(prev: f64, d5: Option<f64>) -> f64 {
    match d5 {
        Some(d5) if d5 > prev => d5,
        Some(d5) => 0.8 * prev + 0.2 * d5,
        None => QUIET_KEEP * prev,
    }
}

/// Walk raw prints at the short range's cadence without treating historical bars as samples.
struct Last5sEval {
    next: usize,
    slot_ms: Option<i64>,
    prints: VecDeque<usize>,
    window: Extremes,
    value: Option<f64>,
}

impl Last5sEval {
    /// Start at the first complete print's cadence slot, leaving bar-only history unanswered.
    fn new(series: &Series) -> Self {
        let next = series
            .items
            .iter()
            .position(|i| i.is_print())
            .unwrap_or(series.items.len());
        Self {
            next,
            slot_ms: series
                .items
                .get(next)
                .map(|i| (i.end_ms + SLOT_MS - 1).div_euclid(SLOT_MS) * SLOT_MS),
            prints: VecDeque::new(),
            window: Extremes::new(),
            value: None,
        }
    }

    /// Advance occupied slots and jump quiet runs, retaining the last print before the left edge.
    fn at(&mut self, series: &Series, at: i64) -> Option<f64> {
        let items = &series.items;
        while let Some(slot) = self.slot_ms.filter(|&slot| slot <= at) {
            // Bars never sample this cadence, so a long gap costs one decay, not one per slot.
            while self.next < items.len() && !items[self.next].is_print() {
                self.next += 1;
            }
            if items.get(self.next).is_none_or(|item| item.end_ms > slot) {
                let until_print = items.get(self.next).map_or(i64::MAX, |item| {
                    (item.end_ms - slot + SLOT_MS - 1) / SLOT_MS
                });
                let empty = ((at - slot) / SLOT_MS + 1)
                    .min(until_print)
                    .min(i64::from(i32::MAX));
                // The None branch of last5s_step is the recurrence this power composes.
                self.value = Some(self.value.unwrap_or(0.0) * QUIET_KEEP.powi(empty as i32));
                self.slot_ms = Some(slot + empty * SLOT_MS);
                continue;
            }
            let mut printed = false;
            while self.next < items.len() && items[self.next].end_ms <= slot {
                if items[self.next].is_print() {
                    self.prints.push_back(self.next);
                    self.window.push(self.next, items);
                    printed = true;
                }
                self.next += 1;
            }
            let range = if printed {
                let edge = slot - WINDOW_MS;
                // Keep one record strictly before the edge along with everything after it.
                while self.prints.get(1).is_some_and(|&i| items[i].from_ms < edge) {
                    self.prints.pop_front();
                }
                self.prints
                    .front()
                    .and_then(|&i| self.window.range_at(slot, slot - items[i].from_ms, items))
            } else {
                None
            };
            self.value = Some(last5s_step(self.value.unwrap_or(0.0), range));
            self.slot_ms = Some(slot + SLOT_MS);
        }
        self.value
    }
}

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
    /// The short range evolves between track boundaries, including slots without prints.
    last5s: Last5sEval,
}

impl<'a> CoinEval<'a> {
    /// Start the candle windows and the independent short-range cadence.
    pub(super) fn new(series: &'a Series) -> Self {
        Self {
            series,
            windows: std::array::from_fn(|_| Extremes::new()),
            next: 0,
            next_closed: 0,
            hour_first: 0,
            last5s: Last5sEval::new(series),
        }
    }

    /// The coin's fields at a boundary, over what printed before it. Called at ascending
    /// boundaries; d5s advances through every intervening 250 ms slot.
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
        let d5s = self.last5s.at(series, at);
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
