//! Notional labels inside the trade bubbles: a disc big enough prints its trade's quote notional.
//!
//! The discs themselves are the trade layer's (`crosses.hlsl` and its siblings, sized by
//! `moon_chart::bubbles`); this pass only finds the ones a label fits and prints over them, the
//! largest trades first. It reads the resident trade mirror the cursor readout already reads, so no
//! source lock is taken on the present path.

use super::*;

/// How much smaller than the order-label font a bubble label prints, logical pixels: it has to fit
/// inside a disc.
const BUBBLE_LABEL_SHRINK: f32 = 3.0;

/// The smallest size a bubble label prints at, however small the label slider is set.
const BUBBLE_LABEL_MIN_FONT: f32 = 7.0;

/// Labels one pane prints at most. The largest trades come first, so a crowded tape keeps its
/// walls labelled and drops the rest.
const BUBBLE_LABEL_MAX: usize = 200;

/// Slots in a pane's bubble-label run pool; a label's slot comes from its screen position, so a
/// label that stays put keeps its shaped run.
const BUBBLE_RUN_SLOTS: u32 = 256;

/// A monospace glyph's advance as a share of its size, for sizing a label before shaping it.
const MONO_ADVANCE: f32 = 0.62;

/// The fewest glyphs a notional prints with (`5.0`): the label every disc must at least hold.
const SHORTEST_LABEL_GLYPHS: f32 = 3.0;

/// The bubble label's font size for a label-slider setting.
fn bubble_label_font_px(label_font_delta: f32) -> f32 {
    (label_font_px(label_font_delta) - BUBBLE_LABEL_SHRINK).max(BUBBLE_LABEL_MIN_FONT)
}

impl RenderState {
    /// Print the quote notional inside every visible bubble big enough to hold it.
    ///
    /// Does nothing unless the pane draws bubbles. A trade too small for even the shortest label is
    /// skipped by one compare against the notional that label needs, so a dense tape costs a scan,
    /// not a format per trade. Labels that would overlap one already placed are dropped.
    ///
    /// Args:
    ///     ctx: Text context of the current pass.
    ///     idx: Pane index.
    ///     sf: Physical pixels per logical pixel of the text layer.
    ///
    /// Errors:
    ///     Propagates a failed text draw.
    pub(super) fn draw_bubble_labels(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
        idx: usize,
        sf: f32,
    ) -> anyhow::Result<()> {
        let view = {
            let pr = &self.panes[idx];
            if pr.tick_style.shape[0] < 0.5 {
                return Ok(());
            }
            pr.view
        };
        if !(view.time_to_px > 0.0 && view.marker_half > 0.0) {
            return Ok(());
        }
        let fp = bubble_label_font_px(self.label_font_delta);
        let line_h = fp + 2.0;
        let glyph_w = fp * MONO_ADVANCE;
        let max_r = view.marker_half / sf;
        // The radius the shortest label needs; no smaller disc can hold any label at all.
        let shortest_r = {
            let half_w = SHORTEST_LABEL_GLYPHS * glyph_w * 0.5 + 1.0;
            let half_h = line_h * 0.5 + 1.0;
            (half_w * half_w + half_h * half_h).sqrt()
        };
        let Some(min_notional) = moon_chart::bubbles::notional_for_frac(shortest_r / max_r) else {
            return Ok(());
        };
        let (left, top) = (view.bounds[0], view.bounds[1]);
        let (right, bottom) = (left + view.bounds[2], top + view.bounds[3]);
        let from = f64::from(view.view_time0);
        let to = from + f64::from(view.bounds[2] / view.time_to_px);
        let ink = color(self.readout_label);
        let pr = &mut self.panes[idx];
        let mut found = std::mem::take(&mut pr.bubble_found);
        found.clear();
        for c in pr.layers.tick_samples(from, to) {
            let notional = c.price * c.qty;
            // A non-finite notional has no figure to print, whatever disc it got.
            if c.side >= 2 || !(notional >= min_notional) || !notional.is_finite() {
                continue;
            }
            // The shader's own placement: whole physical pixels from the plot's left and bottom.
            let sx = (left + (c.time_rel - view.view_time0) * view.time_to_px).round();
            let sy = (bottom - (c.price - view.view_price0) * view.price_to_px).round();
            if sx < left || sx > right || sy < top || sy > bottom {
                continue;
            }
            found.push((sx / sf, sy / sf, notional));
        }
        // Largest first: a crowd keeps its biggest trades labelled.
        found.sort_unstable_by(|a, b| b.2.total_cmp(&a.2));
        let mut placed = std::mem::take(&mut pr.bubble_placed);
        placed.clear();
        let mut result = Ok(());
        for &(x, y, notional) in &found {
            if placed.len() >= BUBBLE_LABEL_MAX {
                break;
            }
            let text = fmt_amount(notional);
            let w = text.chars().count() as f32 * glyph_w;
            let r = max_r * moon_chart::bubbles::radius_frac(notional);
            if !moon_chart::bubbles::label_fits(r, w, line_h) {
                continue;
            }
            let rect = [x - w * 0.5, y - line_h * 0.5, x + w * 0.5, y + line_h * 0.5];
            let overlaps = placed.iter().any(|p: &[f32; 4]| {
                rect[0] < p[2] && rect[2] > p[0] && rect[1] < p[3] && rect[3] > p[1]
            });
            if overlaps {
                continue;
            }
            placed.push(rect);
            let key = (x.to_bits() ^ y.to_bits().rotate_left(16)) % BUBBLE_RUN_SLOTS;
            let mut slot = key as usize;
            if pr.bubble_runs.len() <= slot {
                pr.bubble_runs
                    .resize_with(slot + 1, GpuCanvasTextRun::default);
            }
            if let Err(err) = draw_sized_text_run(
                &mut pr.bubble_runs,
                &mut slot,
                ctx,
                &text,
                x,
                y,
                0.5,
                0.5,
                ink,
                fp,
                line_h,
                FontWeight::NORMAL,
            ) {
                result = Err(err);
                break;
            }
        }
        pr.bubble_found = found;
        pr.bubble_placed = placed;
        result
    }
}
