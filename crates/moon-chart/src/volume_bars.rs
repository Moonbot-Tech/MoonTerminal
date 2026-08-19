//! Geometry and scaling for the BOTTOM VOLUME band: the per-candle bars or Moonbot-style filled
//! "hills" drawn along the plot's lower edge, plus the max/average reference lines that give the
//! band a readable scale.
//!
//! # Why the geometry lives here
//!
//! `moon-ui-gpui` is a binary crate with no `[lib]`, so nothing there can be reached from a test —
//! its invariants can only be grepped as text. Everything with an oracle worth asserting therefore
//! lives in this crate, exactly as [`crate::trade_marks`] and [`crate::news_marks`] already do, and
//! `chartdx` keeps only the adapter that retains the samples and resolves theme colours.
//!
//! # Two volume bands, not one
//!
//! The chart already draws a per-TRADE volume band from the tick ring, inside the combo texture.
//! This module is the per-CANDLE band, drawn in the base pass and therefore UNDER it. They are
//! separate features with separate opacity settings and are deliberately not unified: one answers
//! "how big was that print", the other "how much traded in this bucket".
//!
//! # Sizes
//!
//! Every constant below is in LOGICAL pixels and is multiplied by the device scale exactly once, at
//! the point of use in the chartdx adapter — the same rule [`crate::trade_marks`] states.

use moon_core::market::ChartCandle;
use moon_core::market::candles::candle_intersects_window;

/// Widest single bar in the `BARS` style, logical pixels.
pub const VOLUME_BAR_W_PX: f32 = 3.0;

/// Thickness of the max and average reference lines, logical pixels.
pub const VOLUME_SCALE_LINE_PX: f32 = 1.0;

/// Narrowest band fraction a hand-edited `theme.toml` can ask for.
pub const VOLUME_HEIGHT_MIN: f32 = 0.02;
/// Widest band fraction. Beyond roughly half the plot the band stops being a footer and starts
/// competing with the price action it is supposed to annotate.
pub const VOLUME_HEIGHT_MAX: f32 = 0.45;

/// One candle reduced to what the volume band needs.
///
/// The adapter retains these rather than reading the shared history buffer: that buffer is cleared
/// on entry to every read and refilled only when the candle series REVISION changed, so during a
/// plain pan — when the visible window moves but the series does not — it is empty, and stats taken
/// from it would blank the band on exactly the gesture that should rescale it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeSample {
    /// Bucket opening time, absolute Unix milliseconds.
    pub t_open_ms: f64,
    /// This bucket's OWN timeframe in milliseconds. The history tail mixes coarser buckets into the
    /// series, and they are wider on screen than the selected timeframe, so visibility has to be
    /// judged per candle rather than against one series-wide width.
    pub tf_ms: f64,
    /// Bucket volume in the base currency.
    pub volume: f32,
}

/// Max and mean bucket volume over the candles currently on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeStats {
    pub max: f32,
    pub avg: f32,
    pub count: u32,
}

/// Reduce a candle slice to the samples the band needs.
///
/// `tf_ms` is the parallel per-candle timeframe array that `ChartHistoryBuffers` carries; an entry
/// of zero or a short array falls back to the series timeframe, matching how the candle upload
/// resolves widths.
pub fn collect_samples(
    candles: &[ChartCandle],
    tf_ms: &[f32],
    series_tf_ms: f64,
    out: &mut Vec<VolumeSample>,
) {
    out.clear();
    out.reserve(candles.len());
    out.extend(candles.iter().enumerate().map(|(i, c)| {
        let own = tf_ms.get(i).copied().unwrap_or(0.0) as f64;
        VolumeSample {
            t_open_ms: c.t_open_ms,
            tf_ms: if own > 0.0 { own } else { series_tf_ms },
            volume: c.volume.max(0.0),
        }
    }));
}

/// Max and average volume over the samples intersecting `[from_ms, to_ms]`.
///
/// Uses `moon_core`'s [`candle_intersects_window`], the same predicate the chart's auto-Y range
/// uses, so the band and the price scale never disagree about which candles are on screen.
///
/// `None` when nothing is visible or every visible bucket is empty — the caller must not build a
/// reciprocal from a zero maximum.
pub fn visible_volume_stats(
    samples: &[VolumeSample],
    from_ms: f64,
    to_ms: f64,
) -> Option<VolumeStats> {
    let mut max = 0.0f32;
    let mut sum = 0.0f64;
    let mut count = 0u32;
    for s in samples {
        if !candle_intersects_window(s.t_open_ms, s.tf_ms, from_ms, to_ms) {
            continue;
        }
        max = max.max(s.volume);
        sum += s.volume as f64;
        count += 1;
    }
    if count == 0 || max <= 0.0 {
        return None;
    }
    Some(VolumeStats {
        max,
        avg: (sum / count as f64) as f32,
        count,
    })
}

/// Clamp a band height fraction read from a hand-edited `theme.toml`.
pub fn clamp_band_fraction(frac: f32) -> f32 {
    if frac.is_finite() {
        frac.clamp(VOLUME_HEIGHT_MIN, VOLUME_HEIGHT_MAX)
    } else {
        VOLUME_HEIGHT_MIN
    }
}

/// Quantize the `1 / visible_max` the shader normalises against.
///
/// The live-edge candle's volume grows with every print, so an exact reciprocal changes on almost
/// every frame. The band is drawn into the CACHED base texture, so a value that never repeats would
/// rebake that texture continuously while the chart sits still — a frame-rate regression that reads
/// as "the chart got slow" and shows up only as `base_bake` tracking `base_blit`.
///
/// Quantizing to a relative step keeps the reciprocal stable through ordinary live-edge growth while
/// still tracking a real rescale. The step is relative rather than absolute because volumes span
/// many orders of magnitude between markets.
pub fn quantize_inv_max(inv_max: f32) -> f32 {
    if !inv_max.is_finite() || inv_max <= 0.0 {
        return 0.0;
    }
    // ~0.4% steps: finer than the eye can read off a 72 px band, coarse enough that a tick landing
    // in the newest bucket does not move it.
    const STEPS_PER_OCTAVE: f32 = 256.0;
    let log = inv_max.log2() * STEPS_PER_OCTAVE;
    (log.round() / STEPS_PER_OCTAVE).exp2()
}

/// Quantize a 0..1 ratio destined for the same uniform, for the same reason.
///
/// The average moves with every print exactly as the maximum does, so a raw ratio beside a
/// quantized reciprocal defeats the quantization entirely: the uniform still differs on every
/// frame, the diff gate still fires, and the base texture still rebakes. Absolute steps rather than
/// relative ones, because this is a bounded fraction and not a magnitude.
///
/// 1/256 of a band is well under one pixel at any height a pane can give it, so the average line
/// does not visibly move.
pub fn quantize_ratio(ratio: f32) -> f32 {
    if !ratio.is_finite() {
        return 0.0;
    }
    const STEPS: f32 = 256.0;
    (ratio.clamp(0.0, 1.0) * STEPS).round() / STEPS
}

#[cfg(test)]
mod tests;
