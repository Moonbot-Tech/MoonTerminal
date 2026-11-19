//! `RoundLevels`: round prices near the last close, on a grid derived from the price's order of
//! magnitude.
//!
//! With the last valid close `c` and `major = 10^floor(log10 c)`, the grid step is `major`
//! (`Major`), `major / 2` (`Half`) or `major / 10` (`Minor`), and every multiple of the step within
//! `range_pct` of `c` on either side is a level. The levels are drawn across the whole chart.
//!
//! # Integers, not accumulated floats
//!
//! Every level is an INTEGER count `k` of tenths of `major` (the finest grid), `k · 10^(e-1)` with
//! `e = floor(log10 c)`; the close itself is between 10 and 100 such tenths. The price of each `k`
//! is computed on its own — `k · 10^n` for a non-negative power, `k / 10^-n` for a negative one,
//! the power built by exact multiplications of ten — so `0.000123`'s grid gives `0.00012`, not
//! `0.000120000000001`, and no error carries from one level to the next. The exponent is checked
//! against the exact powers on both sides, because `log10` of an exact power of ten may land just
//! under the integer.
//!
//! # Strength — roundness
//!
//! `1 + (trailing decimal zeros of k) + (0.5 when what is left is a multiple of 5)`: a `Minor`
//! level scores 1, a `Half` one 1.5, a `Major` one 2 or more, and a level that is round at a
//! coarser order still scores higher (on a close of 0.95, 1.00 scores 3 against 0.90's 2). So
//! `TakeOrder::Strongest` takes the roundest first and falls back to the nearest among equals.
//!
//! At most [`MAX_LEVELS`] levels, the nearest to the close: an unsanitized range cannot turn a
//! bar close into a loop over billions of multiples. Cost O(levels).

use moon_core::config::auto_figures::{DetectorKind, DetectorParams, RoundGrid, RoundLevelsParams};
use moon_core::market::candles::ChartCandle;

use super::{DetectCtx, Detector, Found, FoundShape, bar_ok, pct_frac, positive};

/// Most levels one run returns. `sanitize` bounds the range to 50 %, which on a `Minor` grid is at
/// most about a hundred levels; this only guards a range that skipped it.
pub const MAX_LEVELS: i64 = 256;

/// The `RoundLevels` registry row.
pub struct RoundLevels;

impl Detector for RoundLevels {
    fn kind(&self) -> DetectorKind {
        DetectorKind::RoundLevels
    }

    fn detect(
        &self,
        bars: &[ChartCandle],
        params: &DetectorParams,
        _ctx: &DetectCtx,
    ) -> Vec<Found> {
        let DetectorParams::RoundLevels(p) = params else {
            return Vec::new();
        };
        let Some(close) = bars
            .iter()
            .rev()
            .find(|b| bar_ok(b))
            .and_then(|b| positive(f64::from(b.close)))
        else {
            return Vec::new();
        };
        round_levels(close, p)
    }
}

/// `10^n` built from exact multiplications, so every power up to `10^22` is exact.
fn pow10(n: u32) -> f64 {
    (0..n).fold(1.0, |acc, _| acc * 10.0)
}

/// `k · 10^n` for an integer `k`, rounded once.
fn scaled(k: i64, n: i32) -> f64 {
    if n >= 0 {
        k as f64 * pow10(n.unsigned_abs())
    } else {
        k as f64 / pow10(n.unsigned_abs())
    }
}

/// `floor(log10(price))`, corrected against the exact powers of ten around it. `None` for a price
/// no grid can be built for (outside the range `f32` candles can carry).
fn magnitude(price: f64) -> Option<i32> {
    let guess = price.log10().floor();
    if !guess.is_finite() || guess.abs() > 60.0 {
        return None;
    }
    let mut e = guess as i32;
    // `log10` can land one off either way next to an exact power; one step fixes it.
    if scaled(1, e + 1) <= price {
        e += 1;
    }
    if scaled(1, e) > price {
        e -= 1;
    }
    Some(e)
}

/// Roundness of `k` tenths of a major step; see the module doc.
fn roundness(k: i64) -> f32 {
    let mut rest = k;
    let mut zeros = 0u8;
    while rest != 0 && rest % 10 == 0 && zeros < 18 {
        rest /= 10;
        zeros += 1;
    }
    let half = if rest % 5 == 0 { 0.5 } else { 0.0 };
    1.0 + f32::from(zeros) + half
}

/// The round levels around `close`, ascending.
fn round_levels(close: f64, p: &RoundLevelsParams) -> Vec<Found> {
    let Some(e) = magnitude(close) else {
        return Vec::new();
    };
    // In tenths of the major step the close lies in [10, 100).
    let unit_exp = e - 1;
    let in_units = close / scaled(1, unit_exp);
    let step: i64 = match p.grid {
        RoundGrid::Major => 10,
        RoundGrid::Half => 5,
        RoundGrid::Minor => 1,
    };
    let r = pct_frac(p.range_pct);
    let (lo, hi) = (close * (1.0 - r), close * (1.0 + r));
    let stepf = step as f64;
    let centre = (in_units / stepf).round();
    // Multiples of the step, as counts of steps; clamped to the nearest `MAX_LEVELS` around the
    // close and to positive prices, before converting to integers.
    let first = (in_units * (1.0 - r) / stepf)
        .ceil()
        .max(centre - (MAX_LEVELS / 2) as f64)
        .max(1.0);
    let last = (in_units * (1.0 + r) / stepf)
        .floor()
        .min(centre + (MAX_LEVELS / 2) as f64);
    if !(first.is_finite() && last.is_finite()) || last < first {
        return Vec::new();
    }
    (first as i64..=last as i64)
        .filter_map(|n| {
            let k = n * step;
            let price = scaled(k, unit_exp);
            // The integer bounds came from a float division; the price check is the one that
            // decides, so a level a rounding let in just outside the range stays out.
            (price >= lo && price <= hi && price > 0.0).then(|| Found {
                shape: FoundShape::Horizontal { price },
                touches: Vec::new(),
                strength: roundness(k),
                // No anchor in time: a round price is a level across the whole chart.
                first_ms: 0.0,
                last_ms: 0.0,
                broken: None,
                touch_volume: 0.0,
                tolerance_frac: 0.0,
                tag: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
