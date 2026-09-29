//! Live-edge clock correction: where "now" is on the core's clock, as seen from this PC.
//!
//! The live edge follows the LOCAL clock, while every tick carries the CORE's trade time. When
//! the PC clock lags the core, the newest ticks sit to the right of the edge; the right margin is
//! only 10% of the window (0.3 s at super zoom), so a lag of a few hundred milliseconds already
//! runs them under the order book. This estimator learns the offset between the newest trade
//! times and the local clock as trades arrive, and the live edge is placed at `now + offset`.
//!
//! One estimator belongs to one chart source (core and market); a new source starts from zero.

#[cfg(test)]
mod tests;

/// A sample further than this from the local clock is not a clock offset but a stale or replayed
/// trade (history backfill, a market that was silent for a long time), and is ignored.
/// A PC clock off by more than this is therefore not corrected; the edge stays on the local clock.
pub const MAX_CLOCK_OFFSET_MS: f64 = 10.0 * 60_000.0;

/// Samples the median runs over: wide enough that one outlier never moves the estimate, short
/// enough that a clock correction on the PC is followed within a few trades.
const WINDOW: usize = 9;

/// Offset of the core's trade clock relative to the local clock, in milliseconds.
///
/// Each fresh trade gives one sample `trade_time - local_now`; the estimate is the median of the
/// most recent samples. Delivery latency makes every sample slightly low, so the newest tick lands
/// on the live edge rather than past it, inside the right margin.
#[derive(Debug, Clone, Default)]
pub struct LiveClockOffset {
    /// Newest trade time seen so far; only a trade newer than this is a fresh arrival.
    newest_ms: Option<f64>,
    samples: [f64; WINDOW],
    len: usize,
    next: usize,
    offset_ms: f64,
}

impl LiveClockOffset {
    /// Feed the source's newest trade time at local time `now_ms` and return the current offset.
    ///
    /// The first trade seen only sets the baseline: it may be history of a quiet market, so its
    /// age says nothing about the clock. A time that does not advance adds no sample, so a stale
    /// market keeps the last estimate instead of dragging the edge back.
    pub fn observe(&mut self, newest_trade_ms: Option<f64>, now_ms: f64) -> f64 {
        let Some(newest) = newest_trade_ms.filter(|t| t.is_finite()) else {
            return self.offset_ms;
        };
        if !now_ms.is_finite() {
            return self.offset_ms;
        }
        let fresh = self.newest_ms.is_some_and(|prev| newest > prev);
        if self.newest_ms.is_none_or(|prev| newest > prev) {
            self.newest_ms = Some(newest);
        }
        if !fresh {
            return self.offset_ms;
        }
        let sample = newest - now_ms;
        if sample.abs() > MAX_CLOCK_OFFSET_MS {
            return self.offset_ms;
        }
        self.samples[self.next] = sample;
        self.next = (self.next + 1) % WINDOW;
        self.len = (self.len + 1).min(WINDOW);
        self.offset_ms = median(&self.samples[..self.len]);
        self.offset_ms
    }

    /// Local time `now_ms` translated to the core's trade clock: where the live edge belongs.
    pub fn edge_ms(&self, now_ms: f64) -> f64 {
        now_ms + self.offset_ms
    }
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = [0.0; WINDOW];
    let sorted = &mut sorted[..values.len()];
    sorted.copy_from_slice(values);
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}
