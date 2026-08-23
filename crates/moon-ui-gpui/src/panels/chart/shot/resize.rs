//! Normalizing the captured chart to a size a messenger will not resample again.
//!
//! The capture itself is lossless and 1:1 (`super::win`), and it stays that way. What this module
//! adds is a SINGLE, deliberate downscale with a good filter, taken once on our side so the
//! messenger's own resampler has nothing left to do. A messenger's downscale is a box filter
//! applied to a picture it knows nothing about; ours is Lanczos3 applied to a picture we know is
//! full of 1px candle wicks and small axis text.
//!
//! Platform-neutral on purpose, like `super::rect`: it is pure arithmetic over a byte buffer, so
//! its unit tests run on every platform even though only the Windows arm produces a frame today.

use image::imageops::FilterType;
use image::{ImageBuffer, Rgb};

/// The largest side the FINAL composed picture may have.
///
/// **Why 1280.** It is the size the messenger photo path is chosen against: Telegram's standard
/// photo path resamples anything whose LARGEST SIDE exceeds roughly 1280 px, so handing it a
/// picture already inside that box leaves its resampler nothing to do. That is the whole
/// requirement — the degradation this feature exists to remove is the messenger's own downscale,
/// not ours. It also keeps the PNG small enough that sending it as a document instead is not a
/// nuisance.
///
/// **Largest SIDE, not width, and that distinction is load-bearing.** Capping the width alone lets
/// a tall or portrait chart out at 1280x1600, which the messenger then resamples on the HEIGHT —
/// the exact damage this constant exists to prevent, arrived at through the rule meant to prevent
/// it. A chart slot is usually wider than tall, so the width is usually the binding side;
/// "usually" is not a guarantee, and a narrow docked pane or a tall group window is an ordinary
/// layout rather than an exotic one.
///
/// **Why downscaling is the common case.** A chart slot on a 1440p or 4K monitor at 125–200%
/// scaling captures at roughly 1800–3000 physical pixels wide, so most shots land here — which is
/// exactly where a good filter earns its keep over the box filter a messenger would apply.
///
/// **Why this is a constant and not a setting.** A number the user has to understand in order to
/// choose is a decision handed back to them; there is one right answer for the stated purpose and
/// it lives here, revisable in one place when a messenger changes.
pub(super) const NORMALIZED_MAX_PX: u32 = 1280;

/// Font height for a strip, as a fraction of the picture's width.
///
/// A strip sized in absolute pixels would be a banner on a small pane and a whisper on a wide one.
/// Tying it to the width keeps the line the same visual weight at every size the rule can produce.
const FONT_DIVISOR: u32 = 64;

/// Smallest and largest font a strip will use, in pixels.
///
/// The floor keeps a narrow pane's header legible after a messenger has had its way with it; the
/// ceiling stops a very wide capture from growing a headline — and it is also what BOUNDS
/// [`HEADER_RESERVE_PX`], so it is load-bearing twice.
const FONT_MIN_PX: u32 = 13;
const FONT_MAX_PX: u32 = 22;

/// Blank space above and below the text inside a strip, as a fraction of the BASE font height.
const STRIP_PADDING_NUM: u32 = 2;
const STRIP_PADDING_DEN: u32 = 3;

/// The LEAD font — the one the coin is set in — as a fraction of the base font.
///
/// The coin is the picture's subject and the only field set larger than the rest; every other
/// field shares the base size and is separated from it by WEIGHT and CONTRAST instead. `19/16` is
/// ~1.19x: enough to read as a heading at 13 px, not enough to become a banner at 22 px. A larger
/// ratio was rejected because the strip's height — and therefore [`HEADER_RESERVE_PX`], and
/// therefore how much of the CHART survives the size rule — is driven entirely by this number.
const LEAD_NUM: u32 = 19;
const LEAD_DEN: u32 = 16;

/// The rule under the strip, in pixels.
///
/// One pixel, and it is a HAIRLINE on purpose: it is what makes the caption read as a caption
/// rather than as text floating over the chart, and anything thicker starts competing with the
/// chart's own grid for the reader's attention.
pub(super) const HAIRLINE_PX: u32 = 1;

/// The font a strip is drawn in, for a picture `width` pixels across.
///
/// Lives HERE, in the platform-neutral module, rather than beside the drawing code that uses it —
/// because the size rule below has to know how tall the strip will be in order to leave room for
/// it, and a second copy of this arithmetic in the Windows-only module is exactly the drift
/// [`HEADER_RESERVE_PX`] depends on not happening.
///
/// Args:
///     width: The picture's final width in pixels.
///
/// Returns:
///     The character height to ask GDI for.
pub(super) const fn font_px(width: u32) -> u32 {
    let scaled = width / FONT_DIVISOR;
    if scaled < FONT_MIN_PX {
        FONT_MIN_PX
    } else if scaled > FONT_MAX_PX {
        FONT_MAX_PX
    } else {
        scaled
    }
}

/// The font the COIN is set in, for a strip whose base font is `base`.
///
/// Args:
///     base: Character height in pixels, from [`font_px`].
///
/// Returns:
///     The lead character height to ask GDI for.
pub(super) const fn lead_px(base: u32) -> u32 {
    base * LEAD_NUM / LEAD_DEN
}

/// Blank space above and below the strip's text.
///
/// Extracted from [`strip_height`] rather than left inline because the drawing pass needs the SAME
/// number to place the baseline: the top padding is where the lead font's ascent starts from, so a
/// second copy of this fraction over there is exactly the drift [`HEADER_RESERVE_PX`] depends on
/// not happening.
///
/// Args:
///     base: Character height in pixels, from [`font_px`].
///
/// Returns:
///     The padding above, and equally below, the text.
pub(super) const fn strip_pad(base: u32) -> u32 {
    base * STRIP_PADDING_NUM / STRIP_PADDING_DEN
}

/// Space between two adjacent fields of one group, as a fraction of the base font.
const GAP_NUM: u32 = 5;
const GAP_DEN: u32 = 4;

/// Space between two GROUPS, as a fraction of the base font.
///
/// Exactly twice the field gap, and that ratio is the entire grouping mechanism: it is wide enough
/// that the eye reads three clusters instead of eight tokens, and narrow enough that it never looks
/// like an accidental hole in the line. Nothing else marks a group — no rule, no dot, no glyph.
const GROUP_GAP_NUM: u32 = 5;
const GROUP_GAP_DEN: u32 = 2;

/// The gap between two fields of one group.
///
/// Args:
///     base: Character height in pixels, from [`font_px`].
///
/// Returns:
///     The gap in pixels.
pub(super) const fn field_gap(base: u32) -> u32 {
    base * GAP_NUM / GAP_DEN
}

/// The gap that separates two groups.
///
/// Args:
///     base: Character height in pixels, from [`font_px`].
///
/// Returns:
///     The gap in pixels.
pub(super) const fn group_gap(base: u32) -> u32 {
    base * GROUP_GAP_NUM / GROUP_GAP_DEN
}

/// Where the baseline sits inside the lead font's box when the platform will not say.
///
/// `GetTextMetricsW` answers this exactly and is what the drawing pass asks first; this is the
/// fallback for the call failing, and a mis-placed baseline is a far better outcome than refusing
/// to produce a picture. `4/5` is the ordinary ascent share of a Latin UI face — close enough that
/// the failure is invisible, and it is a NUMBER rather than a guess made at the call site.
///
/// Lives here, in the platform-neutral module, for the same reason every other constant in this
/// file does: arithmetic placed in the Windows-only module is arithmetic no test on another
/// platform ever runs.
///
/// Args:
///     lead: The lead character height, from [`lead_px`].
///
/// Returns:
///     The baseline's offset below the text box's top edge.
pub(super) const fn ascent_fallback_px(lead: u32) -> u32 {
    lead * 4 / 5
}

/// How tall one strip is for a given BASE font height.
///
/// The strip holds two sizes, and the taller one sets its height: the coin at [`lead_px`], every
/// other field at `base`. Padding is charged from the BASE font, not the lead one, so a larger
/// coin grows the strip by its own extra pixels and not by a multiple of them.
///
/// Args:
///     base: Character height in pixels, from [`font_px`].
///
/// Returns:
///     The strip's height in pixels: the lead text, symmetric padding, and the hairline rule.
pub(super) const fn strip_height(base: u32) -> u32 {
    lead_px(base) + 2 * strip_pad(base) + HAIRLINE_PX
}

/// Height reserved for the ONE burnt-in header strip, so the COMPOSED picture still fits the box.
///
/// The strip is drawn AFTER the resize, at final resolution, so its height is not part of what
/// [`normalize`] scales — but it IS part of what the messenger measures. The body is therefore
/// fitted into a box shortened by this much.
///
/// **Named for the strip it reserves, not for "the strips".** There is exactly one, above the
/// body; an earlier version reserved two and kept the plural after the second was removed. A
/// constant whose name misdescribes what it covers is how the next reader folds a new strip into
/// it without touching the arithmetic — so the name states which strip, and a second one would
/// have to earn its own.
///
/// **COMPUTED from the same constants the drawing code uses, never restated.** An earlier version
/// hard-coded 100 and explained the arithmetic in prose in two modules; that closes the invariant
/// only until somebody edits [`FONT_MAX_PX`] and the compiler says nothing. Deriving it from
/// [`strip_height`] at the widest font makes the relationship the compiler's problem instead of a
/// comment's, and the bound is exact: `font_px` can never answer more than `FONT_MAX_PX`, so no
/// real strip can be taller than the one measured here.
///
/// **It reads 55 today, and it used to read 50.** The five pixels bought the strip a larger coin
/// ([`lead_px`]) and a hairline rule under it. The DERIVATION is what matters and it did not move:
/// the formula under [`strip_height`] changed, this line did not, so the two cannot disagree about
/// how tall a strip is. The body box is `1280 x 1225` and the composed picture's largest side is
/// still at most [`NORMALIZED_MAX_PX`] — which, with the file save gone, is the whole of the
/// defence against a messenger's own recompression.
pub(super) const HEADER_RESERVE_PX: u32 = strip_height(FONT_MAX_PX);

/// A picture in the layout every image encoder wants: top-down rows, no padding, three bytes per
/// pixel in RGB order.
///
/// Exactly what [`super::win::DibImage::to_rgb_top_down`] already produces, which is why this
/// module can be platform-neutral while the capture beside it is not.
pub(super) struct RgbFrame {
    pub(super) width: u32,
    pub(super) height: u32,
    /// `width * height * 3` bytes, top-down, RGB.
    pub(super) rgb: Vec<u8>,
}

/// Scale `frame` down so the COMPOSED picture fits the messenger's box, or hand it back untouched.
///
/// The body is fitted into `NORMALIZED_MAX_PX` wide by `NORMALIZED_MAX_PX - HEADER_RESERVE_PX`
/// tall, so that once `super::paint_win` has added the header strip above it, the finished
/// picture's largest side is still inside [`NORMALIZED_MAX_PX`]. Fitting the width alone would let a tall
/// chart out over the box on its height and hand the messenger exactly the resample this whole
/// path exists to avoid.
///
/// **Never upscales, and that is argued rather than assumed.** Enlarging invents no information,
/// makes the PNG bigger, and softens the axis and order-book text the shot exists to carry — it is
/// strictly worse and strictly larger at the same time. A small pane is small because of the
/// user's layout; that is a fact to photograph, not a defect to paper over.
///
/// The identity branch is load-bearing rather than an optimization: it keeps the "already lossless
/// and 1:1" property of the capture path bit-for-bit intact for every shot already inside the box.
/// A mandatory round trip through a resampler would quietly break that for the one case where
/// there was nothing to fix.
///
/// ONE scale factor is applied to both axes. Never crop, never letterbox, never fit by distortion:
/// a squeezed chart lies about candle geometry, which in a trading picture is a correctness bug
/// and not a cosmetic one.
///
/// Args:
///     frame: The captured picture, top-down RGB.
///
/// Returns:
///     `frame` unchanged when it already fits, otherwise a Lanczos3 downscale of it. A frame whose
///     buffer does not match its dimensions is returned untouched: there is no sane resample of an
///     inconsistent buffer, and refusing to guess keeps the clipboard's picture rather than
///     replacing it with a smear.
pub(super) fn normalize(frame: RgbFrame) -> RgbFrame {
    let Some((width, height)) = fitted(frame.width, frame.height) else {
        return frame;
    };
    let expected = frame.width as usize * frame.height as usize * 3;
    if frame.rgb.len() != expected {
        return frame;
    }
    let Some(source) = ImageBuffer::<Rgb<u8>, _>::from_raw(frame.width, frame.height, frame.rgb)
    else {
        // Unreachable given the length check above; `from_raw` hands the buffer back only by
        // consuming it, so there is nothing left to return and a fresh empty frame would be worse
        // than the smallest valid one.
        return RgbFrame {
            width: 0,
            height: 0,
            rgb: Vec::new(),
        };
    };
    let scaled = image::imageops::resize(
        &source,
        width,
        height,
        // Lanczos3 rather than Triangle or CatmullRom: this is typically a ~2x downscale of thin
        // high-contrast strokes, which is the case where a windowed-sinc filter visibly keeps a
        // 1px wick as a wick instead of averaging it into its neighbours.
        FilterType::Lanczos3,
    );
    RgbFrame {
        width,
        height,
        rgb: scaled.into_raw(),
    }
}

/// The size `width x height` must become to fit the body box, or `None` when it already fits.
///
/// Split out as a free function because it is the part of the rule a test can pin without owning a
/// pixel buffer, and because `None` states the never-upscale branch as a value rather than burying
/// it inside a comparison.
///
/// Computed in `f64` and rounded rather than divided in integers: integer arithmetic truncates,
/// and a systematically-short side is a systematically-squeezed chart.
///
/// Args:
///     width: The captured width.
///     height: The captured height.
///
/// Returns:
///     The scaled size, or `None` when the picture already fits or has no area. Neither returned
///     side is ever zero — a picture one pixel across is degenerate but still encodable, while a
///     zero-sided one is not.
pub(super) fn fitted(width: u32, height: u32) -> Option<(u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    let max_height = NORMALIZED_MAX_PX.saturating_sub(HEADER_RESERVE_PX);
    if width <= NORMALIZED_MAX_PX && height <= max_height {
        return None;
    }
    let scale = (f64::from(NORMALIZED_MAX_PX) / f64::from(width))
        .min(f64::from(max_height) / f64::from(height));
    Some((
        (f64::from(width) * scale).round().max(1.0) as u32,
        (f64::from(height) * scale).round().max(1.0) as u32,
    ))
}

#[cfg(test)]
mod tests;
