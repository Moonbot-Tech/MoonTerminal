//! What the model reads off the core's own record rather than off the FAQ: the timing of the
//! core's sell moves, which depends on the machine and the venue the core runs on, not on any
//! strategy field.
//!
//! **The step lag.** The core times each PriceDown step from the moment the previous one went
//! through, so its steps come `PriceDownDelay` plus that core's replace round trip apart, and a
//! chain of them drifts off a schedule of whole delays. Read off the archived Exit lines
//! (2026-09-22, 6 349 consecutive steps): the median lag is 47 ms on GateF, 63 on Bitget1, 16 on
//! BB1, 31 on the BinF cores, 0–1 on F1…F6 — per core, not per venue (two Binance machines sit
//! 30 ms apart). A 30-step chain on a 30 s delay (HEI, PumpsDetection) ends 1.7 s off the
//! schedule, past the point tolerance. The caller calibrates each core from the archived lines
//! it holds ([`step_lag_samples`], [`median_step_lag`]) and hands the result to the model on
//! [`super::Deal::step_lag_ms`].

use super::Deal;
use super::exit::ExitParams;
use super::line::step_ms;
use super::verify::ArchivedExit;

/// Fewest samples a core's lag is taken from; below it the core runs on the plain schedule.
pub const MIN_STEP_LAG_SAMPLES: usize = 5;

/// One deal's PriceDown step-lag samples: for every pair of consecutive archived moves from the
/// first step on, how much later than `PriceDownDelay` the second came, in milliseconds.
///
/// A pair further apart than one and a half delays had a step between them that rounding kept
/// in place and is left out, and so does a pair closer than one delay — a move of another rule
/// (the pump move, a SellLevel) sits between them. The archive's fill point is left out, told
/// apart the way the verdict tells it. Nothing when PriceDown is off.
///
/// Args:
///     deal: The report row — its sale price.
///     exit: The deal's sell parameters, for the PriceDown timer and delay.
///     exit_points: The deal's archived Exit line.
pub fn step_lag_samples(deal: &Deal, exit: &ExitParams, exit_points: &[(i64, f64)]) -> Vec<i64> {
    if exit.price_down_timer_s <= 0.0 || exit.price_down_pct <= 0.0 {
        return Vec::new();
    }
    // The fill point the way the verdict tells it (`verify::ArchivedExit`): a fill through the
    // market lands well past the sale's tolerance of its level and is still no step.
    let moves = ArchivedExit::of(deal, exit, exit_points).moves;
    let delay_ms = step_ms(exit.price_down_delay_s);
    // From the second move on: the first is the take, and the step after it is timed off the
    // take by `PriceDownTimer`, not off a step before it.
    moves
        .windows(2)
        .skip(1)
        .map(|pair| pair[1].0 - pair[0].0 - delay_ms)
        .filter(|lag| (0..delay_ms / 2).contains(lag))
        .collect()
}

/// A core's step lag: the median of its samples — the mean of the two middle ones for an even
/// count — or `None` below [`MIN_STEP_LAG_SAMPLES`].
///
/// Args:
///     samples: Every sample of the core's deals, in any order; sorted in place.
pub fn median_step_lag(samples: &mut [i64]) -> Option<f64> {
    if samples.len() < MIN_STEP_LAG_SAMPLES {
        return None;
    }
    samples.sort_unstable();
    let mid = samples.len() / 2;
    Some(if samples.len().is_multiple_of(2) {
        (samples[mid - 1] + samples[mid]) as f64 / 2.0
    } else {
        samples[mid] as f64
    })
}

#[cfg(test)]
mod tests;
