//! Test-only timing and input hypotheses; none participates in application replay.

use super::*;

/// Independent timing/input changes layered over the batch-one winning formula.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Probe {
    pub units: Option<super::units::Units>,
    pub lag_s: u32,
    pub hold: u32,
    pub high_to_min: bool,
    pub ticker: bool,
    pub low_s: u32,
    pub suppress: bool,
}

/// The winning formula includes the current sample in its current-price mean.
pub(crate) const BEST: DropsVariant = DropsVariant {
    include_current: true,
    wall_slots: false,
    rolling_high: false,
    cadence: DropsCadence::Second,
    delay_slot: false,
};

/// Closed two-second slots appear one second later, using the last print on each side.
pub(crate) fn ticker_tape(ticks: &[Tick]) -> Vec<Tick> {
    let mut result = Vec::new();
    let mut begin = 0;
    while begin < ticks.len() {
        let slot = (ticks[begin].time_ms as i64).div_euclid(SAMPLE_MS) * SAMPLE_MS;
        let end = ticks.partition_point(|t| t.time_ms < (slot + SAMPLE_MS) as f64);
        let mut buy = None;
        let mut sell = None;
        for tick in &ticks[begin..end] {
            if tick.price.is_finite() && tick.price > 0.0 {
                match tick.side {
                    Side::Buy => buy = Some(tick.price),
                    Side::Sell => sell = Some(tick.price),
                }
            }
        }
        if let Some(price) = match (buy, sell) {
            (Some(b), Some(s)) => Some(b / 2.0 + s / 2.0),
            (b, s) => b.or(s),
        } {
            result.push(Tick {
                time_ms: (slot + SAMPLE_MS + 1000) as f64,
                price,
                qty: 1.0,
                side: Side::Buy,
            });
        }
        begin = end;
    }
    result
}

impl Probe {
    /// Use the window minimum as denominator only for the explicitly selected hypothesis.
    pub(crate) fn reading(self, p: &DropsParams, input: &[Tick], t: i64) -> Option<DropReading> {
        let mut r = if let Some(units) = self.units {
            units.reading(p, input, t)?
        } else {
            p.drop_at_variant(input, t, BEST)?
        };
        if self.high_to_min {
            r.last_price = samples(input, t - i64::from(p.max_time_s) * 1000, t)
                .into_iter()
                .map(|(_, price)| price)
                .reduce(f64::min)?;
            r.drop_pct = (r.high_zone_avg / r.last_price - 1.0) * 100.0;
        }
        Some(r)
    }

    /// Require actual held history for a low; an initial seed cannot establish a whole hour.
    pub(crate) fn low_pass(self, p: &DropsParams, ticks: &[Tick], t: i64) -> Option<bool> {
        if !p.price_is_low || self.low_s == 0 {
            return Some(true);
        }
        let from = t - i64::from(self.low_s) * 1000;
        let current = last_price(ticks, t, None)?;
        let seed = last_price(ticks, from, None)?;
        let begin = ticks.partition_point(|tick| tick.time_ms <= from as f64);
        let end = ticks.partition_point(|tick| tick.time_ms <= t as f64);
        let low = ticks[begin..end]
            .iter()
            .filter(|tick| tick.time_ms > from as f64 && tick.time_ms <= t as f64)
            .filter(|tick| tick.price.is_finite() && tick.price > 0.0)
            .map(|tick| f64::from(tick.price))
            .fold(seed, f64::min);
        Some(current <= low)
    }

    /// Reset the consecutive-pass counter on any failed check, including a suppressed pass.
    pub(crate) fn detect(
        self,
        p: &DropsParams,
        input: &[Tick],
        from: i64,
        to: i64,
        suppression: Option<(i64, i64)>,
    ) -> Option<(i64, DropReading)> {
        let mut passes = 0;
        for t in (from..=to).step_by(1000) {
            let reading = self.reading(p, input, t).filter(|r| {
                r.drop_pct >= p.price_delta_pct
                    && self.low_pass(p, input, t) == Some(true)
                    && (!self.suppress
                        || suppression.is_none_or(|(previous, until)| t < previous || t >= until))
            });
            if let Some(r) = reading {
                passes += 1;
                if passes >= self.hold.max(1) {
                    return Some((t, r));
                }
            } else {
                passes = 0;
            }
        }
        None
    }

    /// Score orders on the original tape; a delayed ticker is only the detector's input.
    pub(crate) fn trace(
        self,
        p: &DropsParams,
        deal: &Deal,
        ticks: &[Tick],
        input: &[Tick],
        suppression: Option<(i64, i64)>,
    ) -> DropsTrace {
        let from = deal.buy_set_ms.unwrap_or(deal.buy_ms) - SEARCH_BEFORE_MS;
        let mut trace = DropsTrace {
            refused: !self
                .units
                .map_or_else(|| p.covers(input, from), |u| u.covers(p, input, from)),
            degenerate: self.units.is_none() && p.price_ma_s >= 2 && p.max_time_s < p.price_ma_s,
            ..Default::default()
        };
        if trace.refused || trace.degenerate || !deal.is_long() {
            return trace;
        }
        let Some((t, r)) = self.detect(p, input, from, deal.buy_ms + SEARCH_AFTER_MS, suppression)
        else {
            return trace;
        };
        let order = t + i64::from(self.lag_s) * 1000;
        trace.detect_ms = Some(t);
        trace.order_ms = Some(order);
        trace.reading = Some(r);
        trace.limit = p.buy_price(r, ticks, order, deal.tick.unwrap_or(0.0));
        trace.fill = trace.limit.and_then(|level| fill_from(ticks, order, level));
        trace
    }
}
