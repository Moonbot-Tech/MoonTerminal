//! DropsDetection replay from the tape. The FAQ's drop is
//! `(highest zone average / last averaged price - 1) * 100`, checked every second.
//! Zones slide by one second; exchange prices are sampled every two seconds.
//! The tape approximates ASK by buy-side prints and cannot establish an hourly minimum:
//! `DropsPriceIsLow` is held at the strategy value, never searched or inferred here.
//! Calibration must establish agreement before the entry gate is enabled.

use super::{Deal, Fill, reaches, snap_to_step};
use crate::feed::types::{Side, Tick};

/// Strategy kind spelling; real-data calibration must confirm it before enabling the gate.
pub const KIND_DROPS: &str = "DropsDetection";
/// The exchange-price sampling interval described by the FAQ.
pub const SAMPLE_MS: i64 = 2_000;
/// Maximum entry-price deviation accepted by verification and calibration, in percent.
pub const PRICE_TOLERANCE_PCT: f64 = 0.1;
/// Keep re-timing inside the traded move, rather than searching unrelated earlier drops.
pub const SEARCH_BEFORE_MS: i64 = 60_000;
/// Keep re-timing inside the traded move, allowing a small late detect.
pub const SEARCH_AFTER_MS: i64 = 10_000;

/// Strategy inputs only; hourly-low eligibility is held, not reconstructed from partial tape.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DropsParams {
    pub max_time_s: u32,
    pub price_ma_s: u32,
    pub last_price_ma: u32,
    pub price_delta_pct: f64,
    pub price_is_low: bool,
    pub use_last_price: bool,
    pub buy_price_pct: f64,
    pub buy_price_last_trade: bool,
    pub use_30s_old_ask: bool,
}

/// Detector inputs and result at one decision second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropReading {
    pub high_zone_avg: f64,
    pub last_price: f64,
    pub drop_pct: f64,
}

/// The comment's factual detect, independent of any variant's parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropsDetect {
    pub x_price_delta_pct: f64,
    pub price_is_low: bool,
}

/// Read only a DropsDetection header, avoiding similarly named fields in later comment lines.
pub fn parse_comment(comment: &str) -> Option<DropsDetect> {
    let header = comment
        .trim_start()
        .strip_prefix("DropsDetection:")?
        .lines()
        .next()?;
    let x_price_delta_pct = header
        .split_once("xPriceDelta:")?
        .1
        .split_whitespace()
        .next()?
        .trim_end_matches('%')
        .parse::<f64>()
        .ok()?;
    let price_is_low = match header
        .split_once("PriceIsLow:")?
        .1
        .split_whitespace()
        .next()?
        .to_ascii_lowercase()
        .as_str()
    {
        "true" | "yes" | "1" => true,
        "false" | "no" | "0" => false,
        _ => return None,
    };
    x_price_delta_pct.is_finite().then_some(DropsDetect {
        x_price_delta_pct,
        price_is_low,
    })
}

/// Last valid print at or before a sample endpoint; invalid prices never seed carry-forward.
fn last_price(ticks: &[Tick], t_ms: i64, side: Option<Side>) -> Option<f64> {
    let end = ticks.partition_point(|tick| tick.time_ms <= t_ms as f64);
    ticks[..end]
        .iter()
        .rev()
        .find(|tick| {
            tick.price.is_finite() && tick.price > 0.0 && side.is_none_or(|side| tick.side == side)
        })
        .map(|tick| f64::from(tick.price))
}

/// Last trade per two-second slot aligned to `to_ms`, carrying the preceding price over holes.
pub fn samples(ticks: &[Tick], from_ms: i64, to_ms: i64) -> Vec<(i64, f64)> {
    if from_ms > to_ms {
        return Vec::new();
    }
    let start = to_ms - (to_ms - from_ms) / SAMPLE_MS * SAMPLE_MS;
    (start..=to_ms)
        .step_by(SAMPLE_MS as usize)
        .filter_map(|t| last_price(ticks, t, None).map(|price| (t, price)))
        .collect()
}

impl DropsParams {
    /// Equality ignores no strategy field: changing even a held input requires a model replay.
    pub fn same_strategy(&self, other: &Self) -> bool {
        self == other
    }

    /// History covering detector warm-up and, when used, the thirty-second buy reference.
    pub fn needed_history_ms(&self) -> i64 {
        let detector = (i64::from(self.max_time_s) + i64::from(self.price_ma_s.max(2))) * 1_000
            + i64::from(self.last_price_ma) * SAMPLE_MS;
        if self.use_30s_old_ask && !self.use_last_price {
            detector.max(30_000)
        } else {
            detector
        }
    }

    /// Whether a valid seed price covers every input's required history at this decision.
    fn covers(&self, ticks: &[Tick], t_ms: i64) -> bool {
        ticks
            .iter()
            .find(|tick| tick.price.is_finite() && tick.price > 0.0)
            .is_some_and(|tick| tick.time_ms <= (t_ms - self.needed_history_ms()) as f64)
    }

    /// Read one decision, refusing a tape whose first valid price is after the required start.
    pub fn drop_at(&self, ticks: &[Tick], t_ms: i64) -> Option<DropReading> {
        if !self.covers(ticks, t_ms) {
            return None;
        }
        let last = if self.last_price_ma == 0 {
            last_price(ticks, t_ms, None)?
        } else {
            let values = samples(
                ticks,
                t_ms - i64::from(self.last_price_ma) * SAMPLE_MS,
                t_ms - SAMPLE_MS,
            );
            if values.is_empty() {
                return None;
            }
            values.iter().map(|(_, p)| p).sum::<f64>() / values.len() as f64
        };
        let prices = samples(ticks, t_ms - i64::from(self.max_time_s) * 1_000, t_ms);
        let mut high: f64 = 0.0;
        let width_ms = if self.price_ma_s < 2 {
            0
        } else {
            i64::from(self.price_ma_s) * 1_000
        };
        if self.price_ma_s < 2 {
            // No zone smoothing: the highest raw two-second exchange-price sample.
            high = prices.iter().map(|(_, p)| *p).reduce(f64::max)?;
        } else {
            for end in
                ((t_ms - i64::from(self.max_time_s) * 1_000 + width_ms)..=t_ms).step_by(1_000)
            {
                let (sum, count) = prices
                    .iter()
                    .filter(|(t, _)| *t > end - width_ms && *t <= end)
                    .fold((0.0, 0u32), |(sum, n), (_, p)| (sum + p, n + 1));
                if count > 0 {
                    high = high.max(sum / f64::from(count));
                }
            }
        }
        (high > 0.0).then_some(DropReading {
            high_zone_avg: high,
            last_price: last,
            drop_pct: (high / last - 1.0) * 100.0,
        })
    }

    /// First second crossing the threshold; missing warm-up seconds are never guessed.
    pub fn detect(&self, ticks: &[Tick], from_ms: i64, to_ms: i64) -> Option<(i64, DropReading)> {
        (from_ms..=to_ms).step_by(1_000).find_map(|t| {
            self.drop_at(ticks, t)
                .filter(|r| r.drop_pct >= self.price_delta_pct)
                .map(|r| (t, r))
        })
    }

    /// Place the long limit off the detector's price or trade/ASK proxy, snapped down to step.
    pub fn buy_price(
        &self,
        reading: DropReading,
        ticks: &[Tick],
        t_ms: i64,
        tick_step: f64,
    ) -> Option<f64> {
        let reference = if self.use_last_price {
            reading.last_price
        } else {
            let side = (!self.buy_price_last_trade).then_some(Side::Buy);
            if self.use_30s_old_ask {
                let initial = last_price(ticks, t_ms - 30_000, side)
                    .or_else(|| last_price(ticks, t_ms - 30_000, None));
                ticks
                    .iter()
                    .filter(|tick| {
                        tick.time_ms > (t_ms - 30_000) as f64
                            && tick.time_ms <= t_ms as f64
                            && tick.price.is_finite()
                            && tick.price > 0.0
                            && side.is_none_or(|s| tick.side == s)
                    })
                    .map(|tick| f64::from(tick.price))
                    .chain(initial)
                    .reduce(f64::min)?
            } else {
                last_price(ticks, t_ms, side).or_else(|| last_price(ticks, t_ms, None))?
            }
        };
        let limit = snap_to_step(
            reference * (1.0 + self.buy_price_pct / 100.0),
            tick_step,
            true,
        );
        (limit.is_finite() && limit > 0.0).then_some(limit)
    }
}

/// Share causal limit-fill semantics between production tracing and measurement probes.
fn fill_from(ticks: &[Tick], order_ms: i64, level: f64) -> Option<Fill> {
    let marketable = last_price(ticks, order_ms, None).is_some_and(|price| level >= price);
    let begin = ticks.partition_point(|tick| tick.time_ms < order_ms as f64);
    ticks[begin..]
        .iter()
        .filter(|tick| tick.price.is_finite() && tick.price > 0.0)
        .find(|tick| reaches(f64::from(tick.price), level, true))
        .map(|tick| Fill {
            t_ms: tick.time_ms as i64,
            price: if marketable {
                level.min(f64::from(tick.price))
            } else {
                level
            },
        })
}

/// Trace separates missing history, impossible zones, and covered non-crossing or no fill.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DropsTrace {
    pub detect_ms: Option<i64>,
    pub order_ms: Option<i64>,
    pub reading: Option<DropReading>,
    pub limit: Option<f64>,
    pub fill: Option<Fill>,
    pub refused: bool,
    pub degenerate: bool,
}

/// Long-only detector replay; short-side DropsDetection has no model.
pub struct DropsEntry<'a>(pub &'a DropsParams);

impl DropsEntry<'_> {
    /// Replay detection and first reaching print inside the bounded traded move.
    pub fn trace(&self, deal: &Deal, ticks: &[Tick]) -> DropsTrace {
        let mut trace = DropsTrace::default();
        if !deal.is_long() {
            return trace;
        }
        let from = deal.buy_set_ms.unwrap_or(deal.buy_ms) - SEARCH_BEFORE_MS;
        let to = deal.buy_ms + SEARCH_AFTER_MS;
        trace.refused = !self.0.covers(ticks, from);
        // A missing earlier second could have fired first; a later warm-up cannot repair it.
        if trace.refused {
            return trace;
        }
        trace.degenerate = self.0.price_ma_s >= 2 && self.0.max_time_s < self.0.price_ma_s;
        if trace.degenerate {
            return trace;
        }
        let Some((t_ms, reading)) = self.0.detect(ticks, from, to) else {
            return trace;
        };
        trace.detect_ms = Some(t_ms);
        trace.reading = Some(reading);
        trace.limit = self
            .0
            .buy_price(reading, ticks, t_ms, deal.tick.unwrap_or(0.0));
        trace.order_ms = Some(t_ms);
        trace.fill = trace.limit.and_then(|level| fill_from(ticks, t_ms, level));
        trace
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod probe;

#[cfg(test)]
pub(crate) mod units;

#[cfg(test)]
pub(crate) mod variant;

#[cfg(test)]
pub(crate) use variant::{DropsCadence, DropsVariant};

/// Share the production print lookup with offline measurements.
#[cfg(test)]
pub(crate) fn measurement_last_price(ticks: &[Tick], t_ms: i64) -> Option<f64> {
    last_price(ticks, t_ms, None)
}
