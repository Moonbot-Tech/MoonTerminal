//! The REST ticker the book-watching stop, the stop ladder and the trailing stop read — one clock
//! and one price rule for the three, so they sample the same arrivals.
//!
//! Inside a stretch of the recorded market list (`ticks::quotes`) the ticker IS the list: the
//! arrivals fall on its points, a step of the list period where nothing changed, and the price on
//! a side is the list's unless a print on that side came after its point. Outside one, as before
//! the recording existed: arrivals every `ticker_period_ms` from the walk's own anchor, and the
//! last print on the side standing in for the book.

use std::sync::Arc;

use crate::db::tuner::ticks::quotes::{QuoteTrack, newest};

/// How far past a period the next recorded point may lie and still be taken for the next arrival,
/// as a share of the period: the list comes every ~2 s on most venues (Binance, OKX), the core's
/// ticker every ~2.15 s, so a point within half a period more is the same arrival.
const SNAP_SHARE: f64 = 1.5;

/// The ticker's clock and book for one walk.
#[derive(Clone, Debug)]
pub(crate) struct Ticker {
    quotes: Option<Arc<QuoteTrack>>,
    period_ms: i64,
}

impl Ticker {
    /// Args:
    ///     quotes: The trade's recorded ticker; `None` runs the walk on its prints alone.
    ///     period_ms: The ticker's period (`ModelSettings::ticker_period_ms`), at least 1 ms.
    pub(crate) fn new(quotes: Option<Arc<QuoteTrack>>, period_ms: i64) -> Self {
        Self {
            quotes,
            period_ms: period_ms.max(1),
        }
    }

    /// The period between two arrivals off the record.
    pub(crate) fn period_ms(&self) -> i64 {
        self.period_ms
    }

    /// The first arrival of a walk whose clock is anchored at `grid_first`: the recorded point
    /// that arrival falls on, when one lies within reach, else the anchor itself.
    pub(crate) fn first(&self, grid_first: i64) -> i64 {
        self.snap(grid_first - 1).unwrap_or(grid_first)
    }

    /// The arrival after the one at `at`.
    pub(crate) fn next(&self, at: i64) -> i64 {
        self.snap(at).unwrap_or(at + self.period_ms)
    }

    /// The recorded point after `at` that is the next arrival, if the record holds one in reach.
    fn snap(&self, at: i64) -> Option<i64> {
        let reach = (self.period_ms as f64 * SNAP_SHARE) as i64;
        self.quotes.as_ref()?.next_point(at, at + reach)
    }

    /// The BID at `at`: the recorded one, or the last taker sell after it.
    ///
    /// Args:
    ///     at: The arrival.
    ///     print: The last taker sell before `at`, with its moment.
    pub(crate) fn bid(&self, at: i64, print: Option<(i64, f64)>) -> Option<f64> {
        let quote = self
            .quotes
            .as_ref()
            .and_then(|q| q.at(at))
            .map(|p| (p.t_ms, p.bid));
        newest(quote, print)
    }

    /// The ASK at `at`: the recorded one, or the last taker buy after it.
    pub(crate) fn ask(&self, at: i64, print: Option<(i64, f64)>) -> Option<f64> {
        let quote = self
            .quotes
            .as_ref()
            .and_then(|q| q.at(at))
            .map(|p| (p.t_ms, p.ask));
        newest(quote, print)
    }

    /// The stop's side of the book at `at` — the BID for a long, the ASK for a short.
    pub(crate) fn stop_side(&self, long: bool, at: i64, print: Option<(i64, f64)>) -> Option<f64> {
        if long {
            self.bid(at, print)
        } else {
            self.ask(at, print)
        }
    }
}

#[cfg(test)]
mod tests;
