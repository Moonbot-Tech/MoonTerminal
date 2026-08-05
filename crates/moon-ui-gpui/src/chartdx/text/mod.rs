//! Text emitted by chart `gpu_canvas.prepare_text`: axis labels and cursor readout.
//! This keeps chart-zone text on the retained GPU path instead of repainting the
//! GPUI view tree on every mouse move.

use gpui::{GpuCanvasTextMetrics, Hsla, point, px};

use super::*;

const FONT_SIZE: f32 = 11.5;
pub(super) const LINE_H: f32 = FONT_SIZE + 4.0;
const READOUT_PAD_X: f32 = 5.0;
const READOUT_PAD_Y: f32 = 2.5;
const READOUT_INSET: f32 = 2.0;
// Distance from an order-line label to the line itself (px). Large enough that the badge
// (its bottom/top = dy ± READOUT_PAD_Y) does not cover the line: GAP > READOUT_PAD_Y.
const LABEL_LINE_GAP: f32 = 4.0;
// Insets of the corner caption (the coin, then the core name) from the zone `text::caption`
// resolves for it. The close button occupies the outermost ~26 px of the pane (bounds `right-26`,
// width 22), so a 30 px right inset keeps the caption's own right edge ~4 px clear of it and stops
// text from hiding underneath.
pub(super) const CAPTION_PAD_X: f32 = 30.0;
pub(super) const CAPTION_PAD_Y: f32 = 4.0;
// Gap between the current Y-scale badge and the corner-caption block (badge on the left).
pub(super) const CAPTION_SCALE_GAP: f32 = 8.0;
const FIRETEST_TEXT_FONT_SIZE: f32 = 9.0;
const FIRETEST_TEXT_LINE_H: f32 = 11.0;

fn color(hex: u32) -> Hsla {
    gpui::rgb(hex).into()
}

fn local_offset_sec() -> i64 {
    crate::chartdx::axes::local_offset_sec()
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

/// Rough width of a label before it is shaped, in logical pixels.
///
/// Used only to decide whether the real measurement is worth taking: a label nowhere near the
/// plot's right edge cannot need flipping, and text shaping is the expensive call on this path.
/// Scales with the font slider, unlike a fixed margin, so a large font still flips.
pub(in crate::chartdx) fn rough_label_width(text: &str, label_font_delta: f32) -> f32 {
    text.chars().count() as f32 * label_font_px(label_font_delta) * 0.65
}

/// Formats cumulative order-book notional compactly with a K/M/B/T SI suffix for cursor labels.
fn fmt_amount(v: f32) -> String {
    moon_core::util::fmt::compact_si(v as f64)
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

fn ensure_text_run(runs: &mut Vec<GpuCanvasTextRun>, cursor: usize) {
    if cursor >= runs.len() {
        runs.push(GpuCanvasTextRun::default());
    }
}

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

/// Draws a `draw_text_run` with a custom font size and line height of `size + 4`.
///
/// Used for the large delta from the anchor in broom mode. The externally supplied size is
/// scaled by the settings slider through `label_font_px`.
#[allow(clippy::too_many_arguments)]
fn draw_sized_text_run(
    runs: &mut Vec<GpuCanvasTextRun>,
    cursor: &mut usize,
    ctx: &mut GpuCanvasTextContext<'_>,
    text: &str,
    size: f32,
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
        px(size),
        px(size + 4.0),
        color,
        ax,
        ay,
    )
}

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
mod prepare;
mod runs;

use caption::{CaptionBox, book_zone_left, caption_geom, caption_layout, column};

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
