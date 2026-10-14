//! Which of the core's five-second mini-candles fill the open candle's blank stretch.
//!
//! `Скрыть последних свечей` removes the body of every bucket from the hide boundary on and leaves
//! trades only; the raw dots, though, are just the core's retained ring and often cover the last
//! minutes of that bucket. The prints the ring already folded away survive as five-second
//! mini-candles (a price range, no open or close). They are drawn from the oldest one up to where
//! the dots begin, never over the dots and never left of the hide boundary; past the oldest
//! mini-candle the stretch stays blank, because nothing honest is known there.

use moon_core::market::candles::{PATH_MINI_MS, PathMini};

/// One mini-candle placed on the price plot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathBar {
    /// Open time relative to the chart epoch, in milliseconds.
    pub t_open_rel: f32,
    pub low: f32,
    pub high: f32,
    /// Whether the range's middle is not below the previous mini-candle's; colours the bar the way
    /// a rising candle is coloured.
    pub rising: bool,
}

/// Places the mini-candles that belong in the hidden stretch left of the raw dots.
///
/// Args:
///     minis: Mini-candles, oldest first.
///     hide_start_ms: Absolute open of the first hidden bucket; non-finite means nothing is hidden.
///     dots_left_ms: Absolute time of the oldest raw dot; non-finite means no dots were placed.
///         An append of newly folded mini-candles passes `f64::INFINITY`: those prints already
///         left the ring, so they cannot sit under a dot.
///     epoch_ms: Chart epoch the output is relative to.
///     prev_mid: Middle of the last bar already placed, to colour the first new bar.
///     out: Receives the placed bars (appended).
///
/// Returns:
///     The middle of the last placed bar, or `prev_mid` when none was placed.
pub fn place_path(
    minis: &[PathMini],
    hide_start_ms: f64,
    dots_left_ms: f64,
    epoch_ms: f64,
    prev_mid: Option<f32>,
    out: &mut Vec<PathBar>,
) -> Option<f32> {
    let mut prev = prev_mid;
    if !hide_start_ms.is_finite() || dots_left_ms.is_nan() {
        return prev;
    }
    for mini in minis {
        if mini.t_open_ms < hide_start_ms || mini.t_open_ms + PATH_MINI_MS > dots_left_ms {
            continue;
        }
        let mid = (mini.low + mini.high) * 0.5;
        out.push(PathBar {
            t_open_rel: (mini.t_open_ms - epoch_ms) as f32,
            low: mini.low,
            high: mini.high,
            rising: prev.is_none_or(|p| mid >= p),
        });
        prev = Some(mid);
    }
    prev
}

#[cfg(test)]
mod tests;
