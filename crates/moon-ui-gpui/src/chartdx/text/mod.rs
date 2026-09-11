//! Text emitted by chart `gpu_canvas.prepare_text`: axis labels and cursor readout.
//! This keeps chart-zone text on the retained GPU path instead of repainting the
//! GPUI view tree on every mouse move.

use gpui::{FontWeight, GpuCanvasTextMetrics, Hsla, point, px};

use super::*;

const FONT_SIZE: f32 = 11.5;
pub(super) const LINE_H: f32 = FONT_SIZE + 4.0;
/// Size and weight of the bottom-volume band's max/average scale labels.
///
/// Unlike a price-axis label, which sits in an empty gutter, these two are drawn ON the band, over
/// the bars they annotate. At the axis size and weight they read as part of that texture instead of
/// as its scale, so they take one step up in size and the SEMIBOLD face — the heaviest Geist Mono
/// weight the binary embeds (`startup::embedded_fonts`), so no synthetic emboldening is involved.
const VOLUME_SCALE_FONT_SIZE: f32 = FONT_SIZE + 1.5;
const VOLUME_SCALE_LINE_H: f32 = VOLUME_SCALE_FONT_SIZE + 4.0;
const VOLUME_SCALE_WEIGHT: FontWeight = FontWeight::SEMIBOLD;
/// How much larger the cursor's volume readout is than an order-line label.
///
/// It is read while the pointer moves, against candles rather than against a gutter, and it carries
/// the amounts a reader is comparing digit by digit. It follows the label-size slider like every
/// other chart readout; this is a fixed step on top of it, not a second setting.
pub(super) const READOUT_FONT_BUMP: f32 = 1.5;
const READOUT_PAD_X: f32 = 5.0;
const READOUT_PAD_Y: f32 = 2.5;
const READOUT_INSET: f32 = 2.0;
// Offset of the cursor's mode badge from the crosshair (px): far enough that neither crosshair line
// runs through it, close enough to read as belonging to the cursor.
pub(super) const CURSOR_BADGE_DX: f32 = 10.0;
pub(super) const CURSOR_BADGE_DY: f32 = 8.0;
// Distance from an order-line label to the line itself (px). Large enough that the badge
// (its bottom/top = dy ± READOUT_PAD_Y) does not cover the line: GAP > READOUT_PAD_Y.
const LABEL_LINE_GAP: f32 = 4.0;
// Insets of the corner caption (the coin, then the core name) from the zone `text::caption`
// resolves for it. The close button occupies the outermost ~26 px of the pane (bounds `right-26`,
// width 22), so a 30 px right inset keeps the caption's own right edge ~4 px clear of it and stops
// text from hiding underneath.
pub(super) const CAPTION_PAD_X: f32 = 30.0;
pub(super) const CAPTION_PAD_Y: f32 = 4.0;
const FIRETEST_TEXT_FONT_SIZE: f32 = 9.0;
const FIRETEST_TEXT_LINE_H: f32 = 11.0;

fn color(hex: u32) -> Hsla {
    gpui::rgb(hex).into()
}

fn readout_rect_dst(
    anchor_x: f32,
    anchor_y: f32,
    metrics: GpuCanvasTextMetrics,
    ax: f32,
    ay: f32,
    scale: f32,
) -> [f32; 4] {
    let text_w = metrics.width.as_f32();
    let line_h = metrics.line_height.as_f32();
    let x = anchor_x - text_w * ax - READOUT_PAD_X;
    let y = anchor_y - line_h * ay - READOUT_PAD_Y;
    [
        x * scale,
        y * scale,
        (text_w + READOUT_PAD_X * 2.0) * scale,
        (line_h + READOUT_PAD_Y * 2.0) * scale,
    ]
}

fn rect_x_range_log(dst: [f32; 4], scale: f32) -> (f32, f32) {
    let l = dst[0] / scale;
    (l, l + dst[2] / scale)
}

fn rect_y_range_log(dst: [f32; 4], scale: f32) -> (f32, f32) {
    let t = dst[1] / scale;
    (t, t + dst[3] / scale)
}

/// Returns the gap between cursor labels (size/volume/%) and the crosshair's horizontal line.
///
/// The label badge (text ± `READOUT_PAD_Y`) must not cover the line, which would create a
/// visible break in the cursor. Accounts for crosshair thickness (device px → logical px)
/// with an additional 1 px margin.
fn cursor_label_gap(cursor_thickness_dev: f32, sf: f32) -> f32 {
    LABEL_LINE_GAP.max(READOUT_PAD_Y + cursor_thickness_dev / sf.max(0.1) * 0.5 + 1.0)
}

/// Formats a signed percentage such as `+1.25%` for cursor labels relative to current price.
///
/// Also used by order-line labels (`data_state::orders`).
///
/// Deliberately NOT `moon_core::util::fmt::signed_pct`: this runs per label in the GPU frame
/// path on `f32` cursor deltas, and a deviation that rounds to zero sits at the cursor's own
/// price line, where the sign carries the direction the reader is dragging toward — the ambiguity
/// the shared formatter removes is information here.
pub(in crate::chartdx) fn fmt_pct(v: f32) -> String {
    format!("{v:+.2}%")
}

/// Returns the price a cursor's percentage and color are measured against.
///
/// Moonbot measures from the NEAREST side of the book — best bid below the current price, best
/// ask above it — so the spread shifts the percentage and a long's reference differs from a
/// short's. `book_best` comes from the WHOLE book (`OrderBookModel::best_bid_ask`), never from
/// the pane's visible slice: a reference picked out of whatever levels happen to be on screen
/// travels with the camera, which made the percentage change while the chart was merely panned.
/// Falls back to `last` when the pane holds no book — none for the market yet, or the order book
/// switched off for the window, which clears the field rather than letting it freeze.
///
/// A one-sided book deliberately measures both directions against its single side:
/// `best_bid_ask` reports that side in both positions, and it is a live execution price where
/// `last` may be minutes old. No positivity check here — the producer already rejects a
/// non-finite or non-positive side.
///
/// Shared by the real cursor (`prepare`) and the compare-mode ghost (`runs`) so the same price
/// reads the same percentage on either chart.
pub(in crate::chartdx) fn cursor_ref_price(
    book_best: Option<(f32, f32)>,
    last: f32,
    price: f32,
) -> f32 {
    book_best.map_or(last, |(bid, ask)| if price >= last { ask } else { bid })
}

/// Rough width of a label before it is shaped, in logical pixels.
///
/// Used only to decide whether the real measurement is worth taking: a label nowhere near the
/// plot's right edge cannot need flipping, and text shaping is the expensive call on this path.
/// Scales with the font slider, unlike a fixed margin, so a large font still flips.
///
/// Deliberately an OVER-estimate: under-shooting would skip the measurement for a wide label and
/// let it run past the plot, while over-shooting only costs a measurement that changes nothing.
pub(in crate::chartdx) fn rough_label_width(text: &str, label_font_delta: f32) -> f32 {
    text.chars().count() as f32 * label_font_px(label_font_delta)
}

/// Formats cumulative order-book notional compactly with a K/M/B/T SI suffix for cursor labels.
fn fmt_amount(v: f32) -> String {
    moon_core::util::fmt::compact_si(v as f64)
}

/// Formats a prospective order size with compact lowercase SI suffixes for the cursor label.
fn fmt_prospective_order_size(usd: f64) -> String {
    moon_core::util::fmt::compact_order_size(usd)
}

/// Returns the current chart theme's positive or negative color for a signed percentage.
fn pct_hsla(v: f32, positive: u32, negative: u32) -> Hsla {
    color(if v >= 0.0 { positive } else { negative })
}

fn clamp_anchor(value: f32, min: f32, max: f32) -> f32 {
    if min <= max {
        value.clamp(min, max)
    } else {
        (min + max) * 0.5
    }
}

fn label_font_px(label_font_delta: f32) -> f32 {
    (FONT_SIZE + label_font_delta).clamp(6.0, 40.0)
}

/// Size of the cursor's volume readout: the order-line label size plus [`READOUT_FONT_BUMP`].
///
/// Clamped again after the bump so the step cannot push the largest slider setting past the bound
/// [`label_font_px`] exists to hold.
fn readout_font_px(label_font_delta: f32) -> f32 {
    (label_font_px(label_font_delta) + READOUT_FONT_BUMP).clamp(6.0, 40.0)
}

/// Whether a reference line at `frac` of a `band`-tall band has room for its own label.
///
/// The label is centred on the line, so half of it hangs below; too close to the band floor it
/// would print over the plot's bottom edge. The threshold is derived from the label's own line
/// height rather than fixed, so changing the scale labels' size cannot silently start clipping
/// them.
fn volume_scale_label_fits(band: f32, frac: f32) -> bool {
    band * frac >= VOLUME_SCALE_LINE_H * 0.5
}

/// The chart's mono face at one weight; SEMIBOLD resolves to the embedded GeistMono-600.
fn mono_font(weight: FontWeight) -> gpui::Font {
    let mut font = gpui::font(crate::design::mono());
    font.weight = weight;
    font
}

fn ensure_text_run(runs: &mut Vec<GpuCanvasTextRun>, cursor: usize) {
    if cursor >= runs.len() {
        runs.push(GpuCanvasTextRun::default());
    }
}

/// Draw the volume scale and cursor readout with their explicit size and weight.
fn draw_sized_text_run(
    runs: &mut Vec<GpuCanvasTextRun>,
    cursor: &mut usize,
    ctx: &mut GpuCanvasTextContext<'_>,
    text: &str,
    x: f32,
    y: f32,
    ax: f32,
    ay: f32,
    color: Hsla,
    size: f32,
    line_h: f32,
    weight: FontWeight,
) -> anyhow::Result<GpuCanvasTextMetrics> {
    ensure_text_run(runs, *cursor);
    let run = &mut runs[*cursor];
    *cursor += 1;
    run.draw_aligned(
        ctx,
        point(px(x), px(y)),
        text,
        mono_font(weight),
        px(size),
        px(line_h),
        color,
        ax,
        ay,
    )
}

/// The measuring partner of [`draw_sized_text_run`], taking the same size and weight.
fn measure_sized_text_run(
    runs: &mut Vec<GpuCanvasTextRun>,
    cursor: usize,
    ctx: &GpuCanvasTextContext<'_>,
    text: &str,
    size: f32,
    line_h: f32,
    weight: FontWeight,
) -> GpuCanvasTextMetrics {
    ensure_text_run(runs, cursor);
    runs[cursor].measure(ctx, text, mono_font(weight), px(size), px(line_h))
}

/// Draw ordinary chart text without the volume-specific size or weight adjustment.
fn draw_text_run(
    runs: &mut Vec<GpuCanvasTextRun>,
    cursor: &mut usize,
    ctx: &mut GpuCanvasTextContext<'_>,
    text: &str,
    x: f32,
    y: f32,
    ax: f32,
    ay: f32,
    color: Hsla,
) -> anyhow::Result<GpuCanvasTextMetrics> {
    ensure_text_run(runs, *cursor);
    let run = &mut runs[*cursor];
    *cursor += 1;
    run.draw_aligned(
        ctx,
        point(px(x), px(y)),
        text,
        gpui::font(crate::design::mono()),
        px(FONT_SIZE),
        px(LINE_H),
        color,
        ax,
        ay,
    )
}

/// Measure ordinary chart text using the same metrics as its draw path.
fn measure_text_run(
    runs: &mut Vec<GpuCanvasTextRun>,
    cursor: usize,
    ctx: &GpuCanvasTextContext<'_>,
    text: &str,
) -> GpuCanvasTextMetrics {
    ensure_text_run(runs, cursor);
    runs[cursor].measure(
        ctx,
        text,
        gpui::font(crate::design::mono()),
        px(FONT_SIZE),
        px(LINE_H),
    )
}

/// One bottom-volume scale label, one step larger and SEMIBOLD; see [`VOLUME_SCALE_FONT_SIZE`].
fn draw_volume_scale_text_run(
    runs: &mut Vec<GpuCanvasTextRun>,
    cursor: &mut usize,
    ctx: &mut GpuCanvasTextContext<'_>,
    text: &str,
    x: f32,
    y: f32,
    ax: f32,
    ay: f32,
    color: Hsla,
) -> anyhow::Result<GpuCanvasTextMetrics> {
    draw_sized_text_run(
        runs,
        cursor,
        ctx,
        text,
        x,
        y,
        ax,
        ay,
        color,
        VOLUME_SCALE_FONT_SIZE,
        VOLUME_SCALE_LINE_H,
        VOLUME_SCALE_WEIGHT,
    )
}

/// Draw ordinary order labels with the configured label-size adjustment.
fn draw_label_text_run(
    runs: &mut Vec<GpuCanvasTextRun>,
    cursor: &mut usize,
    ctx: &mut GpuCanvasTextContext<'_>,
    label_font_delta: f32,
    text: &str,
    x: f32,
    y: f32,
    ax: f32,
    ay: f32,
    color: Hsla,
) -> anyhow::Result<GpuCanvasTextMetrics> {
    let fp = label_font_px(label_font_delta);
    ensure_text_run(runs, *cursor);
    let run = &mut runs[*cursor];
    *cursor += 1;
    run.draw_aligned(
        ctx,
        point(px(x), px(y)),
        text,
        gpui::font(crate::design::mono()),
        px(fp),
        px(fp + 4.0),
        color,
        ax,
        ay,
    )
}

/// Measure ordinary order labels with the same mono face and size used for drawing.
fn measure_label_text_run(
    runs: &mut Vec<GpuCanvasTextRun>,
    cursor: usize,
    ctx: &GpuCanvasTextContext<'_>,
    label_font_delta: f32,
    text: &str,
) -> GpuCanvasTextMetrics {
    let fp = label_font_px(label_font_delta);
    ensure_text_run(runs, cursor);
    runs[cursor].measure(
        ctx,
        text,
        gpui::font(crate::design::mono()),
        px(fp),
        px(fp + 4.0),
    )
}

/// The cursor volume readout's own size, above the order-line label size by a fixed step.
fn draw_readout_text_run(
    runs: &mut Vec<GpuCanvasTextRun>,
    cursor: &mut usize,
    ctx: &mut GpuCanvasTextContext<'_>,
    label_font_delta: f32,
    text: &str,
    x: f32,
    y: f32,
    ax: f32,
    ay: f32,
    color: Hsla,
) -> anyhow::Result<GpuCanvasTextMetrics> {
    let fp = readout_font_px(label_font_delta);
    draw_sized_text_run(
        runs,
        cursor,
        ctx,
        text,
        x,
        y,
        ax,
        ay,
        color,
        fp,
        fp + 4.0,
        FontWeight::NORMAL,
    )
}

/// Measure the enlarged volume readout using its draw path's font and line height.
fn measure_readout_text_run(
    runs: &mut Vec<GpuCanvasTextRun>,
    cursor: usize,
    ctx: &GpuCanvasTextContext<'_>,
    label_font_delta: f32,
    text: &str,
) -> GpuCanvasTextMetrics {
    let fp = readout_font_px(label_font_delta);
    measure_sized_text_run(runs, cursor, ctx, text, fp, fp + 4.0, FontWeight::NORMAL)
}

fn nearest_orderbook_notional(
    levels: &[moon_core::data::BookDepthPoint],
    price: f32,
    tol: f32,
) -> Option<f32> {
    fn consider(
        best: &mut Option<(f32, f32)>,
        level: Option<&moon_core::data::BookDepthPoint>,
        price: f32,
        tol: f32,
    ) {
        if let Some(level) = level {
            let d = (level.price - price).abs();
            if d <= tol && best.is_none_or(|(bd, _)| d < bd) {
                *best = Some((d, level.cum_notional));
            }
        }
    }

    let split = levels.partition_point(|level| !level.is_ask);
    let bids = &levels[..split];
    let asks = &levels[split..];
    let mut best = None;

    if !bids.is_empty() {
        let mut lo = 0;
        let mut hi = bids.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            if bids[mid].price > price {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        consider(&mut best, bids.get(lo), price, tol);
        if lo > 0 {
            consider(&mut best, bids.get(lo - 1), price, tol);
        }
    }

    if !asks.is_empty() {
        let ix = asks.partition_point(|level| level.price < price);
        consider(&mut best, asks.get(ix), price, tol);
        if ix > 0 {
            consider(&mut best, asks.get(ix - 1), price, tol);
        }
    }

    best.map(|(_, q)| q)
}

// Split by responsibility: runs contains RenderState draw/measure helpers, FireTest text, and
// the ghost cursor; prepare contains the main prepare_text implementation.
mod caption;
mod captions;
mod labels;
mod prepare;
mod runs;
mod tick_volume;

#[cfg(test)]
mod tests;

pub(in crate::chartdx) use caption::CaptionBox;
use caption::book_zone_left;
pub(in crate::chartdx) use captions::{ActionDraw, CAPTION_PLATES, CaptionBar, CaptionGeomInput};
/// The caption editor lives outside the chart and needs exactly one thing from the text pass:
/// the real formatter, applied to sample values.
pub(crate) use labels::preview_row;
pub(in crate::chartdx) use labels::{
    ActionInputs, ActionMark, BasisStats, LabelInputs, LabelState, collect_open_stats,
};

/// Width of `text` at `size` in the caption font, without touching the retained run list.
///
/// [`crate::design::fit_text`] takes an `Fn` measuring closure, which cannot hold the `&mut self`
/// that [`RenderState::measure_text`] needs, and the caption draws its two rows at two different
/// sizes. Measuring through a throwaway run answers both: it borrows only the context, and it takes
/// the size the caller actually draws at — truncating against a narrower font would underestimate
/// the width and overflow anyway.
fn measure_run_width(ctx: &GpuCanvasTextContext<'_>, text: &str, size: f32) -> f32 {
    GpuCanvasTextRun::default()
        .measure(
            ctx,
            text,
            gpui::font(crate::design::mono()),
            px(size),
            px(size + 4.0),
        )
        .width
        .as_f32()
}
