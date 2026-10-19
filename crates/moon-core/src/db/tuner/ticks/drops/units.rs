//! Measurement-only unit interpretations; production never compiles these knobs.
use super::*;

/// Interpret the current price independently of the high-zone smoothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Current {
    /// The last valid trade at the decision time.
    Print,
    /// The mean of preceding endpoints, excluding the decision endpoint.
    MeanJ,
    /// Include the decision endpoint; implicit units keep the legacy two-second cadence.
    #[default]
    MeanIncludingSlot,
    /// The last buyer-initiated print, used only as an order-book proxy.
    AskProxy,
}

impl Current {
    /// Share the dense diagnostic array index across cached readers and reports.
    pub(crate) fn index(self) -> usize {
        match self {
            Self::Print => 0,
            Self::MeanJ => 1,
            Self::MeanIncludingSlot => 2,
            Self::AskProxy => 3,
        }
    }
}

/// Alternative units plus an optional explicit diagnostic grid point.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Units {
    pub window: u8,
    pub high: u8,
    pub last_seconds: bool,
    pub extend: bool,
    pub explicit: Option<(u32, usize, usize, i64)>,
    pub current: Current,
}

impl Units {
    /// Resolve units without changing the strategy values loaded from the dump.
    pub(crate) fn dimensions(self, p: &DropsParams) -> (u32, usize, usize, i64) {
        if let Some(point) = self.explicit {
            return point;
        }
        let w = match self.window {
            1 => p.max_time_s.saturating_mul(2),
            2 => p.max_time_s.max(30),
            _ => p.max_time_s,
        };
        let step = if self.high == 2 { 1000 } else { 2000 };
        let k = if self.high == 0 {
            (p.price_ma_s as usize).div_ceil(2)
        } else {
            p.price_ma_s as usize
        }
        .max(1);
        let j = if self.last_seconds {
            (p.last_price_ma as usize).div_ceil(2)
        } else {
            p.last_price_ma as usize
        };
        (w, k, j, step)
    }

    /// Require actual warm-up for both zone history and the current mean.
    pub(crate) fn covers(self, p: &DropsParams, ticks: &[Tick], t: i64) -> bool {
        let (w, k, j, step) = self.dimensions(p);
        if k == 0 || step <= 0 {
            return false;
        }
        let history = (i64::from(w)
            + if self.extend {
                k as i64 * step / 1000
            } else {
                0
            })
            * 1000;
        let current_step = if self.explicit.is_some() { step } else { 2000 };
        let oldest = if self.current == Current::MeanJ {
            j
        } else {
            j.saturating_sub(1)
        };
        last_price(ticks, t - history.max(oldest as i64 * current_step), None).is_some()
    }

    /// Average complete zones; extended zones may begin before the high-search window.
    pub(crate) fn reading(self, p: &DropsParams, ticks: &[Tick], t: i64) -> Option<DropReading> {
        if !self.covers(p, ticks, t) {
            return None;
        }
        let (w, k, j, step) = self.dimensions(p);
        let from = t - i64::from(w) * 1000;
        let lower = from - if self.extend { k as i64 * step } else { 0 };
        let start = t - (t - lower) / step * step;
        let values: Option<Vec<f64>> = (0..=((t - start) / step))
            .map(|i| last_price(ticks, start + i * step, None))
            .collect();
        let values = values?;
        let high = values
            .windows(k)
            .enumerate()
            .filter(|(i, _)| {
                let end = start + (*i + k - 1) as i64 * step;
                if self.extend {
                    end >= from
                } else {
                    start + *i as i64 * step > from
                }
            })
            .map(|(_, v)| v.iter().sum::<f64>() / k as f64)
            .reduce(f64::max)?;
        let last = match self.current {
            Current::AskProxy => last_price(ticks, t, Some(Side::Buy))?,
            Current::Print => last_price(ticks, t, None)?,
            Current::MeanJ | Current::MeanIncludingSlot if j == 0 => last_price(ticks, t, None)?,
            Current::MeanJ | Current::MeanIncludingSlot => {
                let current_step = if self.explicit.is_some() { step } else { 2000 };
                let end = t - if self.current == Current::MeanJ {
                    current_step
                } else {
                    0
                };
                (0..j)
                    .map(|i| last_price(ticks, end - i as i64 * current_step, None))
                    .collect::<Option<Vec<_>>>()?
                    .iter()
                    .sum::<f64>()
                    / j as f64
            }
        };
        Some(DropReading {
            high_zone_avg: high,
            last_price: last,
            drop_pct: (high / last - 1.0) * 100.0,
        })
    }
}
