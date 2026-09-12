//! Scaling and selection for the `candle_volume_sides` half of the bottom band: bought and sold
//! turnover as ROLLING sums over the band's own interval, sampled along the time axis and drawn as
//! two filled series the way Moonbot's `Vol` indicator draws them — a print stays in the picture
//! for a whole interval, so the band is a continuous hill rather than a row of columns. Where the
//! trade history does not reach, the band continues from the candle turnover on the same scale;
//! see [`crate::volume_bars::visible_interval_max`].
//!
//! Beside [`crate::volume_bars`] rather than inside it: that module scales ONE figure per candle on
//! a square-root height, this one scales TWO figures per sample on a LINEAR height — Moonbot labels
//! its scale `max` at the top and `max / 2` half-way, which only reads true when height is
//! proportional. The two share the band's height, opacity and reference lines, and nothing else.
//!
//! The samples come from [`moon_core::market::SideVolumeBucket`]; everything here is pure over a
//! slice of them, so it can be asserted in tests the way `volume_bars` is.

use moon_core::market::SideVolumeBucket;
use moon_core::market::candles::candle_intersects_window;

/// Rolling intervals the reader can pick, in seconds — Moonbot's `TimeFrame` list on its `Vol`
/// popup.
///
pub const SIDE_TF_CHOICES_S: [u32; 6] = [5, 15, 30, 60, 180, 300];

/// Moonbot's `Auto` table for the vertical volumes, verbatim: the interval against the VISIBLE
/// span, rounded to whole minutes before the comparison. The candle timeframe plays no part —
/// only how much time fits on screen. Pairs of (minutes visible, at least; interval in seconds).
///
/// The first row asks for two seconds, which the reader cannot pick by hand (the list starts at
/// five): Moonbot's quirk, kept as it states it. The live tail is kept per second, so two seconds
/// is honest there; history older than the trade ring is five-second aggregates and steps coarser.
const AUTO_TF_TABLE: [(u32, u32); 7] = [
    (120, 300),
    (60, 180),
    (40, 60),
    (20, 30),
    (10, 15),
    (5, 5),
    (0, 2),
];

/// Finest sampling of the rolling sums, milliseconds: the native one-second slot.
pub const NATIVE_STEP_MS: i64 = 1_000;

/// How often the rolling sums are sampled at a zoom: once per native slot, or once per screen
/// pixel where a pixel is wider than a slot — a sample the eye cannot separate from its neighbour
/// is an instance for nothing.
///
/// Args:
///     px_per_ms: The view's horizontal scale, LOGICAL pixels per millisecond.
///
/// Returns:
///     A multiple of [`NATIVE_STEP_MS`], at least one.
pub fn sample_step_ms(px_per_ms: f32) -> i64 {
    if !px_per_ms.is_finite() || px_per_ms <= 0.0 {
        return NATIVE_STEP_MS;
    }
    let ms_per_px = (1.0 / px_per_ms) as f64;
    let slots = (ms_per_px / NATIVE_STEP_MS as f64).ceil().max(1.0);
    // Well under any real chart width times a day, and never a NaN reaching `as`.
    (slots.min(1e6) as i64) * NATIVE_STEP_MS
}

/// Snap a configured interval onto the reader's list.
///
/// Zero is `Auto` and stays zero; anything else lands on the nearest listed interval, so a
/// hand-edited `layout.toml` value between two steps lights one segment instead of none and the
/// band sums over the interval the popup shows.
///
/// Args:
///     tf_s: Configured interval in seconds, `0` for `Auto`.
///
/// Returns:
///     `0`, or one of [`SIDE_TF_CHOICES_S`].
pub fn snap_tf_s(tf_s: u32) -> u32 {
    if tf_s == 0 {
        return 0;
    }
    SIDE_TF_CHOICES_S
        .iter()
        .copied()
        .min_by_key(|c| c.abs_diff(tf_s))
        .unwrap_or(SIDE_TF_CHOICES_S[0])
}

/// The interval `Auto` picks for a visible span: the row of [`AUTO_TF_TABLE`] the span reaches.
///
/// Args:
///     visible_ms: How much time the plot shows edge to edge, milliseconds.
///
/// Returns:
///     Interval in milliseconds; the narrowest row when the span is not a usable number.
pub fn auto_tf_ms(visible_ms: f64) -> i64 {
    if !visible_ms.is_finite() || visible_ms <= 0.0 {
        return i64::from(AUTO_TF_TABLE[AUTO_TF_TABLE.len() - 1].1) * 1_000;
    }
    let minutes = (visible_ms / 60_000.0).round().min(u32::MAX as f64) as u32;
    AUTO_TF_TABLE
        .iter()
        .find(|(at_least, _)| minutes >= *at_least)
        .map(|(_, tf_s)| i64::from(*tf_s) * 1_000)
        .unwrap_or(i64::from(AUTO_TF_TABLE[AUTO_TF_TABLE.len() - 1].1) * 1_000)
}

/// The interval the band sums over: the configured one, or `Auto`'s pick for the visible span —
/// never below the native slot. The pane widens it to the sampling step where that is wider (no
/// print may fall between two samples) and names THAT in the readout.
pub fn effective_tf_ms(tf_s: u32, visible_ms: f64) -> i64 {
    let tf = match snap_tf_s(tf_s) {
        0 => auto_tf_ms(visible_ms),
        s => i64::from(s) * 1_000,
    };
    tf.max(NATIVE_STEP_MS)
}

/// The tallest sample on screen, in the quote currency.
///
/// Overlaid, a sample is as tall as its larger side; stacked, as tall as both together — so the
/// same samples normalise differently under the two kinds, and the scale label says so.
///
/// Uses `moon_core`'s [`candle_intersects_window`], the same predicate the candle band uses, so the
/// two bands never disagree about which buckets are on screen.
///
/// `None` when nothing is visible or every visible bucket is empty — the caller must not build a
/// reciprocal from a zero maximum.
pub fn visible_side_max(
    buckets: &[SideVolumeBucket],
    from_ms: f64,
    to_ms: f64,
    stacked: bool,
) -> Option<f32> {
    let mut max = 0.0f32;
    for b in buckets {
        if !candle_intersects_window(b.t_open_ms as f64, b.tf_ms as f64, from_ms, to_ms) {
            continue;
        }
        let buy = b.buy_quote.max(0.0);
        let sell = b.sell_quote.max(0.0);
        let height = if stacked { buy + sell } else { buy.max(sell) };
        if height.is_finite() {
            max = max.max(height);
        }
    }
    (max > 0.0).then_some(max)
}

/// The sample a chart time falls in, for the cursor readout.
///
/// Samples are half-open `[t_open_ms, t_open_ms + tf_ms)` and do not overlap, so the first hit is
/// the answer.
pub fn bucket_at(buckets: &[SideVolumeBucket], t_ms: f64) -> Option<SideVolumeBucket> {
    if !t_ms.is_finite() {
        return None;
    }
    buckets.iter().copied().find(|b| {
        b.tf_ms > 0 && t_ms >= b.t_open_ms as f64 && t_ms < (b.t_open_ms + b.tf_ms) as f64
    })
}

#[cfg(test)]
mod tests;
