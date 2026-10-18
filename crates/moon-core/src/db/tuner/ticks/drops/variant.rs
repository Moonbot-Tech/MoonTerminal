//! Calibration variants are compiled only for tests and offline measurement.

use super::*;

/// Independent calibration knobs; the default preserves the gated production model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DropsVariant {
    pub wall_slots: bool,
    pub include_current: bool,
    pub rolling_high: bool,
    pub cadence: DropsCadence,
    pub delay_slot: bool,
}

/// Decision schedules are hypotheses, independent of the price formula.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DropsCadence {
    #[default]
    Second,
    Slot,
    Print,
}

impl DropsVariant {
    /// The reference port evaluates its slot formula on each engine pass, approximated by prints.
    pub(crate) const ASTER: Self = Self {
        wall_slots: true,
        include_current: true,
        rolling_high: true,
        cadence: DropsCadence::Print,
        delay_slot: false,
    };

    /// Closed slots retain their last observed print; the live slot never sees future prints.
    pub(crate) fn samples(self, ticks: &[Tick], from_ms: i64, to_ms: i64) -> Vec<(i64, f64)> {
        if !self.wall_slots {
            return samples(ticks, from_ms, to_ms);
        }
        wall_samples(ticks, from_ms, to_ms, usize::MAX)
    }
}

/// Jump over complete slots with binary searches so print-driven probes do not rescan every trade.
fn wall_samples(ticks: &[Tick], from_ms: i64, to_ms: i64, limit: usize) -> Vec<(i64, f64)> {
    let lower = from_ms.div_euclid(SAMPLE_MS) * SAMPLE_MS;
    let begin = ticks.partition_point(|tick| tick.time_ms < lower as f64);
    let mut end = ticks.partition_point(|tick| tick.time_ms <= to_ms as f64);
    let mut result = Vec::new();
    while end > begin && result.len() < limit {
        let Some(offset) = ticks[begin..end]
            .iter()
            .rposition(|tick| tick.price.is_finite() && tick.price > 0.0)
        else {
            break;
        };
        let tick = &ticks[begin + offset];
        let slot = (tick.time_ms as i64).div_euclid(SAMPLE_MS) * SAMPLE_MS;
        result.push((slot, f64::from(tick.price)));
        end = ticks[..end].partition_point(|tick| tick.time_ms < slot as f64);
    }
    result.reverse();
    result
}

impl DropsParams {
    /// Read a calibration hypothesis without changing the default production path.
    pub(crate) fn drop_at_variant(
        &self,
        ticks: &[Tick],
        t_ms: i64,
        variant: DropsVariant,
    ) -> Option<DropReading> {
        if !self.covers(ticks, t_ms) {
            return None;
        }
        let last = if self.last_price_ma == 0 {
            last_price(ticks, t_ms, None)?
        } else {
            let end = if variant.include_current {
                t_ms
            } else if variant.wall_slots {
                t_ms.div_euclid(SAMPLE_MS) * SAMPLE_MS - 1
            } else {
                t_ms - SAMPLE_MS
            };
            let values = if variant.wall_slots {
                wall_samples(
                    ticks,
                    ticks.first()?.time_ms as i64,
                    end,
                    self.last_price_ma as usize,
                )
            } else {
                samples(
                    ticks,
                    end - i64::from(self.last_price_ma.saturating_sub(1)) * SAMPLE_MS,
                    end,
                )
            };
            if values.is_empty() {
                return None;
            }
            values.iter().map(|(_, p)| p).sum::<f64>() / values.len() as f64
        };
        let from = t_ms - i64::from(self.max_time_s) * 1_000;
        let mut prices = variant.samples(ticks, from, t_ms);
        if variant.rolling_high {
            // The reference window contains round(max_time / 2) slots including the live slot.
            let count = (f64::from(self.max_time_s) / 2.0).round().max(1.0) as i64;
            let anchor = if variant.wall_slots {
                t_ms.div_euclid(SAMPLE_MS) * SAMPLE_MS
            } else {
                t_ms
            };
            prices.retain(|(t, _)| *t > anchor - count * SAMPLE_MS);
        }
        let mut high: f64 = 0.0;
        let width_ms = if self.price_ma_s < 2 {
            0
        } else {
            i64::from(self.price_ma_s) * 1_000
        };
        if variant.rolling_high {
            let k = ((f64::from(self.price_ma_s) / 2.0).round() as usize)
                .max(1)
                .min(prices.len());
            if k == 0 {
                return None;
            }
            high = prices
                .windows(k)
                .map(|zone| zone.iter().map(|(_, p)| p).sum::<f64>() / k as f64)
                .reduce(f64::max)?;
        } else if self.price_ma_s < 2 {
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

    /// Replay only the selected decision schedule, preserving causal print visibility.
    pub(crate) fn detect_variant(
        &self,
        ticks: &[Tick],
        from_ms: i64,
        to_ms: i64,
        variant: DropsVariant,
    ) -> Option<(i64, DropReading)> {
        let mut decisions: Box<dyn Iterator<Item = i64> + '_> = match variant.cadence {
            DropsCadence::Second => Box::new((from_ms..=to_ms).step_by(1_000)),
            DropsCadence::Slot => {
                let first = from_ms + (-from_ms).rem_euclid(SAMPLE_MS);
                Box::new((first..=to_ms).step_by(SAMPLE_MS as usize))
            }
            DropsCadence::Print => {
                let mut times: Vec<_> = ticks
                    .iter()
                    .map(|t| t.time_ms.ceil() as i64)
                    .filter(move |t| *t >= from_ms && *t <= to_ms)
                    .collect();
                times.dedup();
                Box::new(times.into_iter())
            }
        };
        decisions.find_map(|t| {
            self.drop_at_variant(ticks, t, variant)
                .filter(|r| r.drop_pct >= self.price_delta_pct)
                .map(|r| (t, r))
        })
    }
}

impl DropsEntry<'_> {
    /// Measure a hypothesis while keeping the application's entry gate and defaults untouched.
    pub(crate) fn trace_variant(
        &self,
        deal: &Deal,
        ticks: &[Tick],
        variant: DropsVariant,
    ) -> DropsTrace {
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
        trace.degenerate = !variant.rolling_high
            && self.0.price_ma_s >= 2
            && self.0.max_time_s < self.0.price_ma_s;
        if trace.degenerate {
            return trace;
        }
        let Some((t_ms, reading)) = self.0.detect_variant(ticks, from, to, variant) else {
            return trace;
        };
        trace.detect_ms = Some(t_ms);
        trace.reading = Some(reading);
        let t_ms = t_ms + if variant.delay_slot { SAMPLE_MS } else { 0 };
        trace.order_ms = Some(t_ms);
        trace.limit = self
            .0
            .buy_price(reading, ticks, t_ms, deal.tick.unwrap_or(0.0));
        trace.fill = trace.limit.and_then(|level| fill_from(ticks, t_ms, level));
        trace
    }
}
