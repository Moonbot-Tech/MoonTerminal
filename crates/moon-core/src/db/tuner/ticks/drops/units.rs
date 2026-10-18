//! Measurement-only unit interpretations; production never compiles these knobs.
use super::*;

/// Alternative units plus an optional explicit diagnostic grid point.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Units {
    pub window: u8,
    pub high: u8,
    pub last_seconds: bool,
    pub extend: bool,
    pub explicit: Option<(u32, usize, usize)>,
}

impl Units {
    /// Resolve units without changing the strategy values loaded from the dump.
    pub(crate) fn dimensions(self, p: &DropsParams) -> (u32, usize, usize, i64) {
        if let Some((w, k, j)) = self.explicit {
            return (w, k, j, 2000);
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
        let history = (i64::from(w)
            + if self.extend {
                k as i64 * step / 1000
            } else {
                0
            })
            * 1000;
        last_price(
            ticks,
            t - history.max(j.saturating_sub(1) as i64 * 2000),
            None,
        )
        .is_some()
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
        let last = if j == 0 {
            last_price(ticks, t, None)?
        } else {
            (0..j)
                .map(|i| last_price(ticks, t - i as i64 * 2000, None))
                .collect::<Option<Vec<_>>>()?
                .iter()
                .sum::<f64>()
                / j as f64
        };
        Some(DropReading {
            high_zone_avg: high,
            last_price: last,
            drop_pct: (high / last - 1.0) * 100.0,
        })
    }
}
