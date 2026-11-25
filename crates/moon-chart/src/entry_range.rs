//! The chart's picture of a manual entry price the core moved onto an exchange limit (MoonBot 7.73
//! N2): the allowed range as a translucent band and the originally requested price as a solid
//! line, shown for [`SHOW`] after the notice arrives and faded out over that time.
//!
//! Backend-free on purpose. Every GPU backend draws the result through the order layer it already
//! has — the band as a zone (baked into the base cache, under the candles), the line as a
//! full-width horizontal (drawn on every present, over them) — so DX11, Metal and wgpu cannot
//! drift apart on what the notice looks like or when it changes.
//!
//! The fade is STEPPED, not continuous. The band lives in the base cache, and a continuous fade
//! would re-bake the background, grid, candles and book on every present for three seconds; in
//! [`FADE_STEPS`] steps the notice costs that many re-bakes and then nothing at all — after
//! [`SHOW`] the plan is `None`, and a chart with no notice has no reason left to repaint.

use std::time::Duration;

use crate::layers::{LineInstance, ZoneInstance, rgb_with_alpha};

/// How long a notice stays on the chart after it arrives, the reference terminal's three seconds.
pub const SHOW: Duration = Duration::from_millis(3_000);
/// How many opacity steps the fade takes; each lasts `SHOW / FADE_STEPS`.
pub const FADE_STEPS: u32 = 12;
/// Opacity of the allowed-range band at full strength, the reference's twenty percent.
pub const BAND_OPACITY: f32 = 0.20;
/// Thickness of the requested-price line, in logical pixels.
pub const LINE_PX: f32 = 2.0;
/// How many visible price windows past the plot's edge an open side of the band reaches.
///
/// An open side cannot be a huge sentinel price the way `layers::TIME_UNBOUNDED` is a sentinel
/// time: the zone shaders clamp a band's TIME to the plot but not its price, so a price of 1e12
/// becomes a pixel Y near 1e20, and the quad's far edge computed as `y0 + (y1 - y0)` in `f32`
/// cancels to zero — the band would cover the wrong span or vanish, differently per backend. A few
/// windows past the edge is off screen on every backend and stays a small, exact pixel value. The
/// layer is rebuilt on every fade step, and `instances` also keeps half the requested price as a
/// floor, so only a pan of several windows within one 250 ms step could show the band's end.
pub const OPEN_SIDE_WINDOWS: f64 = 4.0;

const _: () = assert!(SHOW.as_nanos() < moon_core::feed::ENTRY_NOTICE_RETAIN.as_nanos());
const _: () = assert!(SHOW.as_nanos().is_multiple_of(FADE_STEPS as u128));

/// What one notice draws at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntryRangePlan {
    /// The price the entry asked for: the line, and its caption.
    pub requested: f64,
    /// The allowed range as `(low, high)`, an open side as an infinity that [`instances`] resolves
    /// against the visible window; `None` when the core reported no limit at all or limits that do
    /// not form a range.
    pub band: Option<(f64, f64)>,
    /// The fade step this moment falls in, `0..FADE_STEPS`. Equal steps draw the same picture.
    pub step: u32,
    /// Strength of the whole picture, `1.0` at arrival down to `1 / FADE_STEPS` in the last step.
    pub opacity: f32,
    /// Time after arrival at which the picture next changes: the next step, or the expiry.
    pub next_change: Duration,
}

/// The picture of a notice `elapsed` after it arrived, or `None` once it has expired.
///
/// Both limits present form the band between them, and a request outside them is what the core
/// corrected. One limit alone leaves the other side open, so the band runs to that edge of the
/// plot. No limit at all draws no band — a tint over the whole plot would say every price is
/// allowed, which is the opposite of the notice — but the requested line still shows. Inverted
/// limits draw no band either, as the reference draws none when its bottom edge is above its top.
///
/// Args:
///     requested: The requested price; not a finite positive number draws nothing.
///     min: The lower limit, if any.
///     max: The upper limit, if any.
///     elapsed: Time since the notice arrived.
///
/// Returns:
///     The plan, or `None` for an expired or undrawable notice.
pub fn plan(
    requested: f64,
    min: Option<f64>,
    max: Option<f64>,
    elapsed: Duration,
) -> Option<EntryRangePlan> {
    if !(requested.is_finite() && requested > 0.0) || elapsed >= SHOW {
        return None;
    }
    let step_len = SHOW / FADE_STEPS;
    let step = (elapsed.as_nanos() / step_len.as_nanos()) as u32;
    let band = match (min, max) {
        (None, None) => None,
        (lo, hi) => {
            let lo = lo.unwrap_or(f64::NEG_INFINITY);
            let hi = hi.unwrap_or(f64::INFINITY);
            (lo < hi).then_some((lo, hi))
        }
    };
    Some(EntryRangePlan {
        requested,
        band,
        step,
        opacity: 1.0 - step as f32 / FADE_STEPS as f32,
        next_change: step_len * (step + 1),
    })
}

/// The GPU instances of a plan: the band, if it has one, and the requested-price line.
///
/// An open side of the band is placed past the visible edge on its side (or past the band's other
/// edge, if that is further out) by [`OPEN_SIDE_WINDOWS`] visible windows or half the requested
/// price, whichever is more, so it runs off the plot.
///
/// Args:
///     plan: What to draw.
///     visible: The pane's visible price window as `(low, high)`.
///     band_rgb: The band's colour, sRGB.
///     line_rgb: The line's colour, sRGB.
///     scale: Device pixels per logical pixel.
///
/// Returns:
///     The band for the zone layer and the line for the horizontal-line layer.
pub fn instances(
    plan: &EntryRangePlan,
    visible: (f64, f64),
    band_rgb: [u8; 3],
    line_rgb: [u8; 3],
    scale: f32,
) -> (Option<ZoneInstance>, LineInstance) {
    // A window not laid out yet (empty or not finite) falls back to half the requested price on
    // either side of it, which is still a window of ordinary size around the notice.
    let (view_lo, view_hi) = match visible {
        (lo, hi) if lo.is_finite() && hi.is_finite() && hi > lo => (lo, hi),
        _ => (plan.requested * 0.5, plan.requested * 1.5),
    };
    // Half the requested price as a floor: a view not yet fitted (`ChartView` is born at 0 ± 0.5)
    // or zoomed far out between two fade steps would otherwise show the band's end inside the
    // plot. Even then the edge stays a few million pixels off at most, which `f32` still carries
    // to well under a pixel.
    let reach = (OPEN_SIDE_WINDOWS * (view_hi - view_lo)).max(plan.requested * 0.5);
    let band = plan.band.map(|(lo, hi)| {
        let lo = match lo.is_finite() {
            true => lo,
            false => view_lo.min(hi) - reach,
        };
        let hi = match hi.is_finite() {
            true => hi,
            false => view_hi.max(lo) + reach,
        };
        ZoneInstance::full_width(
            lo as f32,
            hi as f32,
            rgb_with_alpha(band_rgb, BAND_OPACITY * plan.opacity),
        )
    });
    let line = LineInstance {
        price: plan.requested as f32,
        color: rgb_with_alpha(line_rgb, plan.opacity),
        style: 0.0,
        thickness: LINE_PX * scale,
    };
    (band, line)
}

/// The caption's colour at the plan's strength: the text layer has no alpha, so the fade is the
/// colour mixed toward the chart background it sits on.
///
/// Args:
///     plan: What to draw.
///     text_rgb: The caption's full-strength colour, sRGB.
///     bg_rgb: The chart background, sRGB.
///
/// Returns:
///     The caption colour as `0xRRGGBB`.
pub fn caption_rgb(plan: &EntryRangePlan, text_rgb: [u8; 3], bg_rgb: [u8; 3]) -> u32 {
    let k = plan.opacity.clamp(0.0, 1.0);
    let mix = |t: u8, b: u8| (f32::from(b) + (f32::from(t) - f32::from(b)) * k).round() as u8;
    u32::from_be_bytes([
        0,
        mix(text_rgb[0], bg_rgb[0]),
        mix(text_rgb[1], bg_rgb[1]),
        mix(text_rgb[2], bg_rgb[2]),
    ])
}

#[cfg(test)]
mod tests;
