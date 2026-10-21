//! Backend-free sizing for the trade bubbles: how big a trade's disc is, and when its notional
//! fits inside it.
//!
//! The size is an ABSOLUTE scale of the trade's quote notional, logarithmic between two fixed
//! amounts — not a share of some window's biggest trade. A relative scale would differ between the
//! backends (DX11 scales its volume bars by the bake window, wgpu by the whole resident ring) and
//! would leave baked history at sizes taken from a maximum that has since moved. A fixed scale
//! reads the same in every backend, never needs a rebake, and lets the text pass size a disc from
//! the trade alone. The shaders read the same constants through `TickStyleGpu`.

use moon_core::config::{ChartGraphicsCfg, TradeKind};

/// The largest disc, as a multiple of the cross's half-size. In bubble mode the chart's marker
/// half-size IS this largest radius, so every cull margin and bake span sized from it covers the
/// biggest disc without a second figure.
///
/// Six crosses wide, so the largest disc holds a four-glyph notional (`2.6k`) at the default
/// trade size.
pub const BUBBLE_MAX_MULT: f32 = 6.0;

/// The smallest disc as a fraction of the largest — still a readable dot.
pub const BUBBLE_MIN_FRAC: f32 = 0.3;

/// `log10` of the notional that gets the smallest disc (10 in the quote currency).
pub const BUBBLE_LOG_LO: f32 = 1.0;

/// `log10` of the notional that gets the largest disc (100k in the quote currency).
pub const BUBBLE_LOG_HI: f32 = 5.0;

/// The marker scale the chart sizes its trade glyphs with: the tab's own, times
/// [`BUBBLE_MAX_MULT`] when it draws bubbles.
pub fn marker_extent_scale(cfg: &ChartGraphicsCfg) -> f32 {
    match cfg.trade_kind {
        TradeKind::Crosses => cfg.marker_scale,
        TradeKind::Bubbles => cfg.marker_scale * BUBBLE_MAX_MULT,
    }
}

/// A trade's disc radius as a fraction of the largest, from its quote notional.
///
/// Mirrors the shaders' `bubble_frac` exactly, down to the edge cases — NaN and anything not above
/// zero get the smallest disc, `+inf` the largest: the text pass places a label by it, so the two
/// disagreeing would print notionals that overflow their discs.
pub fn radius_frac(notional: f32) -> f32 {
    let t = if notional > 0.0 {
        ((notional.log10() - BUBBLE_LOG_LO) / (BUBBLE_LOG_HI - BUBBLE_LOG_LO)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    BUBBLE_MIN_FRAC + (1.0 - BUBBLE_MIN_FRAC) * t
}

/// The smallest quote notional whose disc reaches `frac` of the largest — the inverse of
/// [`radius_frac`]. Lets the text pass skip every trade too small for a label with one compare
/// instead of sizing each.
///
/// Returns:
///     The notional, zero for a `frac` the smallest disc already reaches, and `None` for one no
///     disc reaches.
pub fn notional_for_frac(frac: f32) -> Option<f32> {
    if !(frac <= 1.0) {
        return None;
    }
    let t = ((frac - BUBBLE_MIN_FRAC) / (1.0 - BUBBLE_MIN_FRAC)).max(0.0);
    if t == 0.0 {
        return Some(0.0);
    }
    Some(10f32.powf(BUBBLE_LOG_LO + t * (BUBBLE_LOG_HI - BUBBLE_LOG_LO)))
}

/// Whether a label `text_w` wide and `text_h` tall fits inside a disc of `radius`, all in the same
/// pixels, with a pixel of margin on each side.
pub fn label_fits(radius: f32, text_w: f32, text_h: f32) -> bool {
    let half_w = text_w * 0.5 + 1.0;
    let half_h = text_h * 0.5 + 1.0;
    half_w * half_w + half_h * half_h <= radius * radius
}

#[cfg(test)]
mod tests;
