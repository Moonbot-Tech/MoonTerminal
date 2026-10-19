//! Fixed-cohort, causal per-detect measurements; rule selection belongs to the held-out pass.

use super::{Coverage, DropReading, DropsParams, Tick, logs::LogDetect};
use crate::db::tuner::ticks::drops::{
    units::{Current, Units},
    variant::wall_samples,
};
use std::collections::HashMap;

/// A signed window preserves invalid affine results so they count as misses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FitPoint {
    pub w_s: i32,
    pub k: usize,
    pub j: usize,
    pub step_ms: i64,
    pub extend: bool,
    pub current: Current,
    pub wall_slots: bool,
}

impl FitPoint {
    /// Include warm-up at the earliest causal scan and any extended smoothing zone.
    pub(super) fn history_s(self) -> i64 {
        60 + (i64::from(self.w_s)
            + if self.extend {
                self.k as i64 * self.step_ms / 1000
            } else {
                0
            })
        .max(self.j as i64 * self.step_ms / 1000)
    }
    /// Reject out-of-domain rules before any subtraction or table indexing.
    pub(super) fn valid(self) -> bool {
        (2..=300).contains(&self.w_s)
            && (1..=30).contains(&self.k)
            && self.j <= 6
            && [1000, 2000].contains(&self.step_ms)
            && !(self.wall_slots && self.step_ms == 1000)
    }

    /// Enumerate every candidate without allocating the Cartesian product.
    pub(super) fn grid() -> impl Iterator<Item = Self> {
        (2..=300).flat_map(|w_s| {
            (1..=30).flat_map(move |k| {
                (0..=6).flat_map(move |j| {
                    [1000, 2000].into_iter().flat_map(move |step_ms| {
                        [false, true].into_iter().flat_map(move |extend| {
                            [
                                Current::Print,
                                Current::MeanJ,
                                Current::MeanIncludingSlot,
                                Current::AskProxy,
                            ]
                            .into_iter()
                            .flat_map(move |current| {
                                [false, true]
                                    .into_iter()
                                    .filter(move |wall| !(*wall && step_ms == 1000))
                                    .map(move |wall_slots| Self {
                                        w_s,
                                        k,
                                        j,
                                        step_ms,
                                        extend,
                                        current,
                                        wall_slots,
                                    })
                            })
                        })
                    })
                })
            })
        })
    }
}

/// Up-front exclusions are independent of candidate success and preserve a common denominator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    PartialTape,
    PriceIsLowOn,
    DegenerateParams,
}

/// Each candidate retains both agreement bands and a separate log-conditioned diagnostic.
#[derive(Clone, Copy, Debug)]
pub(super) struct PointError {
    pub drop_error: f64,
    pub ask_error_pct: f64,
    pub drop05: bool,
    pub truncation: bool,
    pub ask005: bool,
    pub ask01: bool,
    pub fire2s: bool,
    pub logged_fire2s: bool,
}

impl PointError {
    /// Missing or invalid readings stay in the cohort with infinite error.
    fn miss() -> Self {
        Self {
            drop_error: f64::INFINITY,
            ask_error_pct: f64::INFINITY,
            drop05: false,
            truncation: false,
            ask005: false,
            ask01: false,
            fire2s: false,
            logged_fire2s: false,
        }
    }

    /// Only causal fire participates in joint agreement.
    pub(super) fn joint(self) -> bool {
        self.drop05 && self.ask005 && self.fire2s
    }
}

/// Bit positions and errors always use the caller's unchanged candidate ordering.
pub(super) struct DetectFit {
    pub refusal: Option<Refusal>,
    pub passing: Vec<u64>,
    pub errors: Vec<PointError>,
    pub infeasible: bool,
}

impl DetectFit {
    /// Invalid indices cannot turn a missing candidate into a hit.
    pub(super) fn passes(&self, index: usize) -> bool {
        self.passing
            .get(index / 64)
            .is_some_and(|bits| bits & (1 << (index % 64)) != 0)
    }
}

/// Coverage includes the largest extended zone at the earliest causal decision.
pub(super) fn envelope(t: i64) -> Coverage {
    Coverage::one((t - 420_000, t + 2000))
}

/// Admit exactly the same closed history interval in fitting and cohort accounting.
pub(super) fn coverage_qualified(covered: &Coverage, t: i64, envelope_s: i64) -> bool {
    coverage_class(covered, &Coverage::one((t - envelope_s * 1000, t + 2000))) == 0
}

/// Classify held intervals consistently even when a fully covered market has no prints.
pub(super) fn coverage_class(covered: &Coverage, required: &Coverage) -> usize {
    if covered.covers(required) {
        0
    } else if covered.clip(required).is_empty() {
        2
    } else {
        1
    }
}

/// Reuse complete-zone prefix maxima across all windows and current interpretations.
struct Decision {
    normal: Vec<Vec<Option<f64>>>,
    extended: Vec<Vec<Option<f64>>>,
    current: [[Option<f64>; 7]; 4],
}

impl Decision {
    /// Build zone maxima once per cadence and second, never per grid point.
    fn new(ticks: &[Tick], t: i64, step: i64, wall_slots: bool) -> Self {
        let wall: Vec<Tick>;
        let input = if wall_slots {
            wall = wall_samples(
                ticks,
                ticks
                    .first()
                    .map_or(t - 362_000, |tick| tick.time_ms as i64),
                t,
                usize::MAX,
            )
            .into_iter()
            .map(|(at, price)| Tick {
                time_ms: at as f64,
                price: price as f32,
                qty: 1.0,
                side: crate::feed::types::Side::Buy,
            })
            .collect();
            &wall[..]
        } else {
            ticks
        };
        let prices: Vec<_> = (0..=360)
            .map(|i| crate::db::tuner::ticks::drops::measurement_last_price(input, t - i * step))
            .collect();
        let mut normal = vec![vec![None; 301]; 31];
        let mut extended = normal.clone();
        for k in 1..=30 {
            let mut maxima = Vec::with_capacity(331);
            let mut best: Option<f64> = None;
            for zone in prices.windows(k) {
                if let Some(sum) = zone.iter().copied().sum::<Option<f64>>() {
                    let mean = sum / k as f64;
                    best = Some(best.map_or(mean, |v| v.max(mean)));
                }
                maxima.push(best);
            }
            for w in 2..=300 {
                let count = ((w as i64 * 1000 + step - 1) / step) as usize;
                if count >= k {
                    normal[k][w] = maxima[count - k];
                }
                extended[k][w] = maxima[(w as i64 * 1000 / step) as usize];
            }
        }
        let mut current = [[None; 7]; 4];
        let print = crate::db::tuner::ticks::drops::measurement_last_price(ticks, t);
        let end = ticks.partition_point(|tick| tick.time_ms <= t as f64);
        let ask = ticks[..end]
            .iter()
            .rev()
            .find(|tick| {
                tick.side == crate::feed::types::Side::Buy
                    && tick.price.is_finite()
                    && tick.price > 0.0
            })
            .map(|tick| f64::from(tick.price));
        current[0] = [print; 7];
        current[3] = [ask; 7];
        for (index, end) in [(1, t - step), (2, t)] {
            current[index] = std::array::from_fn(|j| {
                if j == 0 {
                    print
                } else {
                    (0..j)
                        .map(|i| {
                            crate::db::tuner::ticks::drops::measurement_last_price(
                                input,
                                end - i as i64 * step,
                            )
                        })
                        .sum::<Option<f64>>()
                        .map(|sum| sum / j as f64)
                }
            });
        }
        Self {
            normal,
            extended,
            current,
        }
    }

    /// Read a high only after domain validation and candidate-specific warm-up.
    fn high(&self, point: FitPoint) -> Option<f64> {
        if point.extend {
            self.extended[point.k][point.w_s as usize]
        } else {
            self.normal[point.k][point.w_s as usize]
        }
    }
}

/// Cache high zones by decision time; current prices still use their explicit side/mean rule.
#[derive(Default)]
struct Readings {
    decisions: HashMap<(i64, i64, bool), Decision>,
    first_masks: HashMap<(i64, i64, i64, bool, bool, usize, usize, usize), Vec<u64>>,
}

impl Readings {
    /// Wall slots are reconstructed at each decision so no later print enters an earlier fire.
    fn at(
        &mut self,
        point: FitPoint,
        p: &DropsParams,
        ticks: &[Tick],
        t: i64,
    ) -> Option<DropReading> {
        if !point.valid() {
            return None;
        }
        let units = Units {
            explicit: Some((point.w_s as u32, point.k, point.j, point.step_ms)),
            extend: point.extend,
            current: point.current,
            ..Default::default()
        };
        if !units.covers(p, ticks, t) {
            return None;
        }
        let decision = self
            .decisions
            .entry((t, point.step_ms, point.wall_slots))
            .or_insert_with(|| Decision::new(ticks, t, point.step_ms, point.wall_slots));
        let high = decision.high(point)?;
        let index = point.current.index();
        let last = decision.current[index][point.j]?;
        let drop_pct = (high / last - 1.0) * 100.0;
        (high > 0.0 && last > 0.0 && drop_pct.is_finite()).then_some(DropReading {
            high_zone_avg: high,
            last_price: last,
            drop_pct,
        })
    }

    /// Hoist crossings across windows using monotone maxima and XOR range events.
    fn first(
        &mut self,
        point: FitPoint,
        p: &DropsParams,
        ticks: &[Tick],
        from: i64,
        to: i64,
    ) -> Option<i64> {
        let current = point.current.index();
        let j = point.j;
        let key = (
            from,
            to,
            point.step_ms,
            point.wall_slots,
            point.extend,
            current,
            j,
            point.k,
        );
        if !self.first_masks.contains_key(&key) {
            let mut events = vec![0u64; 302];
            for (bit, s) in (from..=to).step_by(1000).enumerate() {
                let decision = self
                    .decisions
                    .entry((s, point.step_ms, point.wall_slots))
                    .or_insert_with(|| Decision::new(ticks, s, point.step_ms, point.wall_slots));
                let Some(last) = decision.current[current][j].filter(|v| *v > 0.0) else {
                    continue;
                };
                // Warm-up shrinks monotonically with window width; it uses the raw tape.
                let mut lo = 2;
                let mut hi = 301;
                while lo < hi {
                    let w = (lo + hi) / 2;
                    let units = Units {
                        explicit: Some((w as u32, point.k, point.j, point.step_ms)),
                        extend: point.extend,
                        current: point.current,
                        ..Default::default()
                    };
                    if units.covers(p, ticks, s) {
                        lo = w + 1;
                    } else {
                        hi = w;
                    }
                }
                let upper = lo;
                lo = 2;
                hi = upper;
                while lo < hi {
                    let w = (lo + hi) / 2;
                    let crossing = decision
                        .high(FitPoint {
                            w_s: w as i32,
                            ..point
                        })
                        .is_some_and(|high| {
                            let drop = (high / last - 1.0) * 100.0;
                            high > 0.0 && drop.is_finite() && drop >= p.price_delta_pct
                        });
                    if crossing {
                        hi = w;
                    } else {
                        lo = w + 1;
                    }
                }
                if lo < upper {
                    events[lo] ^= 1u64 << bit;
                    events[upper] ^= 1u64 << bit;
                }
            }
            let mut mask = 0;
            for event in &mut events {
                mask ^= *event;
                *event = mask;
            }
            self.first_masks.insert(key, events);
        }
        let mask = self.first_masks[&key][point.w_s as usize];
        (mask != 0).then(|| from + i64::from(mask.trailing_zeros()) * 1000)
    }
}

/// Score an explicit grid on one fixed cohort; prior logs affect diagnostics only.
pub(super) fn fit_detect(
    detect: &LogDetect,
    params: &DropsParams,
    ticks: &[Tick],
    covered: &Coverage,
    previous: Option<&LogDetect>,
    points: &[FitPoint],
) -> DetectFit {
    fit_detect_in_cohort(detect, params, ticks, covered, previous, points, 420)
}

/// The held-out protocol supplies its fit-only frozen envelope without changing denominators.
pub(super) fn fit_detect_in_cohort(
    detect: &LogDetect,
    params: &DropsParams,
    ticks: &[Tick],
    covered: &Coverage,
    previous: Option<&LogDetect>,
    points: &[FitPoint],
    envelope_s: i64,
) -> DetectFit {
    let t = detect.detect_ms;
    let refusal = if params.price_is_low {
        Some(Refusal::PriceIsLowOn)
    } else if params.max_time_s < 2
        || params.price_ma_s > params.max_time_s
        || !params.price_delta_pct.is_finite()
        || params.price_delta_pct <= 0.0
        || params.last_price_ma > 6
    {
        Some(Refusal::DegenerateParams)
    } else if !coverage_qualified(covered, t, envelope_s) {
        Some(Refusal::PartialTape)
    } else {
        None
    };
    let raw_max = crate::db::tuner::ticks::drops::samples(ticks, t - 300_000, t)
        .into_iter()
        .map(|(_, v)| v)
        .reduce(f64::max);
    let infeasible = detect.ask.zip(raw_max).is_some_and(|(ask, max)| {
        ask * (1.0 + detect.x_price_delta / 100.0) > max + max.abs() * 1e-9
    });
    let mut result = DetectFit {
        refusal,
        passing: vec![0; points.len().div_ceil(64)],
        errors: Vec::with_capacity(points.len()),
        infeasible,
    };
    let logged_from = previous
        .filter(|prev| {
            prev.core_label == detect.core_label
                && prev.strategy == detect.strategy
                && super::logs::coin_key(&prev.coin) == super::logs::coin_key(&detect.coin)
                && prev.detect_ms < t
        })
        .map_or(t - 60_000, |prev| (prev.detect_ms + 1000).max(t - 60_000));
    let mut readings = Readings::default();
    for (index, &point) in points.iter().enumerate() {
        let mut error = PointError::miss();
        if refusal.is_none() && point.valid() && point.history_s() <= envelope_s {
            if let Some(reading) = readings.at(point, params, ticks, t) {
                error.drop_error = (reading.drop_pct - detect.x_price_delta).abs();
                error.drop05 = error.drop_error <= 0.05 + 1e-9;
                error.truncation = reading.drop_pct >= detect.x_price_delta - 1e-9
                    && reading.drop_pct < detect.x_price_delta + 0.1;
                if let Some(ask) = detect.ask.filter(|ask| ask.is_finite() && *ask > 0.0) {
                    error.ask_error_pct = (reading.last_price / ask - 1.0).abs() * 100.0;
                    error.ask005 = error.ask_error_pct <= 0.05 + 1e-9;
                    error.ask01 = error.ask_error_pct <= 0.1 + 1e-9;
                }
                error.fire2s = readings
                    .first(point, params, ticks, t - 60_000, t + 2000)
                    .is_some_and(|s| (s - t).abs() <= 2000);
                error.logged_fire2s = readings
                    .first(point, params, ticks, logged_from, t + 2000)
                    .is_some_and(|s| (s - t).abs() <= 2000);
            }
        }
        if error.joint() {
            result.passing[index / 64] |= 1 << (index % 64);
        }
        result.errors.push(error);
    }
    result
}

#[cfg(test)]
mod tests;
