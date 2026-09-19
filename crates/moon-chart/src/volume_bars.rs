//! Geometry and scaling for the BOTTOM VOLUME band: Moonbot-style filled "hills" drawn along the
//! plot's lower edge — the bought/sold split where the trade history reaches, the candle turnover
//! before it — plus the scale bracket (max and max / 2) that gives the band a readable scale.
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
//!
//! # Why the band is QUOTE-denominated
//!
//! Base-currency volume looks huge on a cheap coin and tiny on an expensive one, which makes the
//! band show an asset quantity rather than the money traded — a thin coin at ~0.0001 read "1.28M"
//! for about 128 USDT of real turnover. Quote turnover is what the reference terminal (Moonbot)
//! prints, so the bars and their scale labels use the selected market's monetary unit rather than
//! an arbitrary count of base units. Markets with different quote currencies are not directly
//! comparable.

use moon_core::market::ChartCandle;
use moon_core::market::candles::candle_intersects_window;

/// Thickness of the scale bracket's stem and ticks, logical pixels.
pub const VOLUME_SCALE_LINE_PX: f32 = 2.0;

/// Length of the bracket's three ticks — at the maximum, at the second reference level and on
/// the band floor — logical pixels, measured to the right of the stem.
pub const VOLUME_SCALE_TICK_PX: f32 = 8.0;

/// Gap between a tick's end and the label printed after it, logical pixels.
pub const VOLUME_SCALE_LABEL_GAP_PX: f32 = 3.0;

/// The bracket's inset from the plot's LEFT edge, logical pixels — Moonbot's `Ind. Pos` left.
pub const VOLUME_SCALE_LEFT_INSET_PX: f32 = 4.0;

/// The bracket's distance from the plot's RIGHT edge, logical pixels — Moonbot's `Ind. Pos` right.
///
/// NOT mirrored from the left inset: Moonbot stands the bracket well inside the plot, and the
/// distance is a constant of the reference — 218 and 223 px off the plot's right edge on two
/// captures at different window widths, unchanged by zoom — rather than a share of the width or
/// a distance from the last candle. The labels still print to the right of the stem, so on the
/// right side they read into the empty margin before the book.
pub const VOLUME_SCALE_RIGHT_INSET_PX: f32 = 220.0;

/// Quads one scale draw issues: the stem, then the ticks at the maximum, at the second reference
/// level and on the floor. Every backend's draw call and every backend's vertex shader agree on
/// this count through here.
pub const VOLUME_SCALE_INSTANCES: u32 = 4;

/// Horizontal offset of the scale bracket's stem from the plot's LEFT edge, logical pixels.
///
/// One rule for the text pass and the three shaders: the pass places the labels from it, the
/// shaders receive it as the signed offset the uniform carries and apply the same clamp. A plot
/// too narrow for the right-hand inset keeps the bracket inside its edges rather than off them.
///
/// Args:
///     plot_w: Plot width in logical pixels.
///     right: Whether the reader put the scale at the plot's right edge.
///
/// Returns:
///     The stem's x as an offset from the plot's left edge, clamped to the plot.
pub fn scale_bracket_offset(plot_w: f32, right: bool) -> f32 {
    let raw = if right {
        plot_w - VOLUME_SCALE_RIGHT_INSET_PX
    } else {
        VOLUME_SCALE_LEFT_INSET_PX
    };
    raw.clamp(0.0, plot_w.max(0.0))
}

/// The signed inset the shaders take for [`scale_bracket_offset`]: non-negative measures from the
/// plot's left edge, negative from its right. Logical pixels; the caller scales to physical.
pub fn scale_bracket_signed_inset(right: bool) -> f32 {
    if right {
        -VOLUME_SCALE_RIGHT_INSET_PX
    } else {
        VOLUME_SCALE_LEFT_INSET_PX
    }
}

/// Narrowest band fraction a hand-edited chart configuration can ask for.
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
    /// This bucket's turnover in the QUOTE currency — a real figure carried on the candle, or an
    /// estimate where the source had none, but never a base-volume proxy. See the module doc for
    /// why the band reads turnover rather than base volume.
    pub quote_volume: f32,
}

/// The band's scale over the candles currently on screen, in the quote currency: the visible
/// maximum and the figure the second reference line prints.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeStats {
    pub max: f32,
    pub avg: f32,
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
            quote_volume: c.quote_volume.max(0.0),
        }
    }));
}

/// The retained bucket a chart time falls in, for the cursor readout.
///
/// Buckets are half-open `[t_open_ms, t_open_ms + tf_ms)`. The history tail mixes coarser buckets
/// into the series, so membership is judged against each candle's OWN width, exactly as
/// [`visible_interval_max`] judges visibility. Where a coarse filler and a fine bucket both cover
/// the instant, the NARROWEST wins: it is the more precise statement about that moment, and a
/// readout that named the coarse total while the chart drew the fine bar would contradict the bar
/// under the pointer.
///
/// The turnover it carries may be a source figure or an estimate — see [`VolumeSample`] — and this
/// function does not distinguish them, because the sample does not carry the distinction.
///
/// Args:
///     samples: Retained per-candle samples, in any order.
///     t_ms: Absolute Unix milliseconds under the cursor.
///
/// Returns:
///     The covering bucket, or `None` when the time falls in a gap or is not finite.
pub fn sample_at(samples: &[VolumeSample], t_ms: f64) -> Option<VolumeSample> {
    if !t_ms.is_finite() {
        return None;
    }
    let mut best: Option<VolumeSample> = None;
    for s in samples {
        if !s.t_open_ms.is_finite()
            || !s.tf_ms.is_finite()
            || s.tf_ms <= 0.0
            || t_ms < s.t_open_ms
            || t_ms >= s.t_open_ms + s.tf_ms
        {
            continue;
        }
        if best.is_none_or(|b| s.tf_ms < b.tf_ms) {
            best = Some(*s);
        }
    }
    best
}

/// A bucket duration as the chart's own timeframe controls spell it: `30s`, `1m`, `4h`, `1d`.
///
/// The readout has to name the period its figure covers: the history tail mixes coarser buckets
/// into the series, so the same pane can show a one-minute total beside a one-day one, and an
/// unlabelled amount would leave the two indistinguishable. The token is deliberately untranslated,
/// like every other market/technical token (`locales/README.md`).
///
/// Args:
///     tf_ms: Bucket width in milliseconds.
///
/// Returns:
///     The token, or `None` for a width below one millisecond or not finite — a period that cannot
///     be named honestly gets no name rather than a rounded one.
pub fn bucket_label(tf_ms: f64) -> Option<String> {
    if !tf_ms.is_finite() || tf_ms < 1.0 {
        return None;
    }
    let secs = (tf_ms / 1000.0).round() as i64;
    Some(match secs {
        s if s > 0 && s % 86_400 == 0 => format!("{}d", s / 86_400),
        s if s > 0 && s % 3_600 == 0 => format!("{}h", s / 3_600),
        s if s > 0 && s % 60 == 0 => format!("{}m", s / 60),
        s if s > 0 => format!("{s}s"),
        // Sub-second buckets round to zero seconds; name them in milliseconds rather than as `0s`.
        _ => format!("{}ms", tf_ms.round() as i64),
    })
}

/// Whether a stored `(candle_volume_style, candle_volume_sides)` pair means the band is ON.
///
/// The band is one switch: ON is Moonbot's `Vol` on hills, OFF is no band. Every pair an older
/// build could write — bars or hills with the split off, the split alone over an OFF candle half,
/// the retired "sides" style id, a hand-typed number past the last id — reads as ON when it drew
/// ANY volume, because each of those users had the band on and asked to see it; only OFF with the
/// split off reads as OFF. The one rule the normaliser, the renderer's own clamp and the popup's
/// checkbox all derive from, so a stored file can never light a picture the chart is not drawing.
///
/// Args:
///     style: Configured style id from a hand-editable config.
///     sides: The configured bought/sold switch.
///
/// Returns:
///     Whether the band draws.
pub fn volume_band_on(style: u8, sides: bool) -> bool {
    style != moon_core::market::candles::VOLUME_STYLE_OFF || sides
}

/// The style id the band draws with for a stored pair: hills when it is on, OFF otherwise.
///
/// Lives here rather than beside the other `ChartGraphicsCfg` clamps because this module owns the
/// bottom band; `moon_core::market::candles` owns the ids themselves.
///
/// Args:
///     style: Configured style id from a hand-editable config.
///     sides: The configured bought/sold switch.
///
/// Returns:
///     `VOLUME_STYLE_HILLS` or `VOLUME_STYLE_OFF`.
pub fn clamp_volume_style(style: u8, sides: bool) -> u8 {
    if volume_band_on(style, sides) {
        moon_core::market::candles::VOLUME_STYLE_HILLS
    } else {
        moon_core::market::candles::VOLUME_STYLE_OFF
    }
}

/// The tallest candle on screen once its turnover is read as a ROLLING-INTERVAL figure, in the
/// quote currency — the scale the `candle_volume_sides` band shares between its two halves.
///
/// A candle's turnover covers its own timeframe; the sides half covers `interval_ms`. Read on one
/// scale, a candle counts as `turnover × interval / tf` — the turnover an interval of it would have
/// held had the candle traded evenly. An estimate, but the only way the candle history and the
/// split can share one maximum and one label.
///
/// Only candles that OPEN before `boundary_ms` count — the same test the shaders cull on, so the
/// candle straddling the boundary, which is drawn (the split half covers its tail), scales the
/// band too; past the boundary the split half draws, and a candle there would scale the band
/// against a figure nobody sees.
///
/// Args:
///     samples: Retained per-candle samples.
///     from_ms: Visible window start, unix milliseconds.
///     to_ms: Visible window end, unix milliseconds.
///     interval_ms: The sides band's rolling interval, milliseconds.
///     boundary_ms: Where the split history begins, unix milliseconds.
///
/// Returns:
///     The maximum, or `None` when no candle before the boundary is visible or all are empty.
pub fn visible_interval_max(
    samples: &[VolumeSample],
    from_ms: f64,
    to_ms: f64,
    interval_ms: f64,
    boundary_ms: f64,
) -> Option<f32> {
    if !(interval_ms > 0.0) {
        return None;
    }
    let mut max = 0.0f32;
    for s in samples {
        if !(s.tf_ms > 0.0)
            || s.t_open_ms >= boundary_ms
            || !candle_intersects_window(s.t_open_ms, s.tf_ms, from_ms, to_ms)
        {
            continue;
        }
        let scaled = (f64::from(s.quote_volume) * interval_ms / s.tf_ms) as f32;
        if scaled.is_finite() {
            max = max.max(scaled);
        }
    }
    (max > 0.0).then_some(max)
}

/// Clamp a bottom-volume band height from a hand-editable chart configuration.
///
/// The ONE authority for this number: `moon_chart::trade_marks::normalize_chart_graphics` calls it
/// rather than declaring a range of its own, and the renderer calls it directly. A second range
/// anywhere would rewrite stored values the chart already draws correctly.
///
/// Args:
///     frac: Configured band-height fraction; a non-finite value is unusable.
///
/// Returns:
///     The fraction in `[VOLUME_HEIGHT_MIN, VOLUME_HEIGHT_MAX]`, or the minimum when it is not
///     finite.
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
