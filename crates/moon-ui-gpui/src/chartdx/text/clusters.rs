//! Volume labels inside the trade bubbles: a disc big enough prints its volume.
//!
//! The discs are the userdata layer's (folded, sized and merged in `data_state::clusters`); this
//! pass only finds the visible ones a label fits and prints over them, the biggest first. Every
//! disc it reads — position, volume, radius — the sync already holds, so a present costs a walk
//! over the current zoom's discs and no source read.

use super::*;

/// How much smaller than the order-label font a bubble label prints, logical pixels: it has to
/// fit inside a disc.
const CLUSTER_LABEL_SHRINK: f32 = 4.0;

/// The smallest size a bubble label prints at, however small the label slider is set.
const CLUSTER_LABEL_MIN_FONT: f32 = 7.0;

/// Labels one pane prints at most. The biggest discs come first, so a crowded tape keeps its walls
/// labelled and drops the rest.
const CLUSTER_LABEL_MAX: usize = 200;

/// Slots in a pane's bubble-label run pool; a label's slot comes from its disc, so a label that
/// stays on its disc keeps its shaped run.
const CLUSTER_RUN_SLOTS: i64 = 256;

/// A monospace glyph's advance as a share of its size, for sizing a label before shaping it.
const MONO_ADVANCE: f32 = 0.62;

/// The bubble label's font size for a label-slider setting.
fn cluster_label_font_px(label_font_delta: f32) -> f32 {
    (label_font_px(label_font_delta) - CLUSTER_LABEL_SHRINK).max(CLUSTER_LABEL_MIN_FONT)
}

impl RenderState {
    /// Print the volume inside every visible bubble big enough to hold it.
    ///
    /// Does nothing unless the pane draws bubbles. Small prints — dots — never carry a label, and
    /// labels that would overlap one already placed are dropped.
    ///
    /// Args:
    ///     ctx: Text context of the current pass.
    ///     idx: Pane index.
    ///     sf: Physical pixels per logical pixel of the text layer.
    ///
    /// Errors:
    ///     Propagates a failed text draw.
    pub(super) fn draw_cluster_labels(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
        idx: usize,
        sf: f32,
    ) -> anyhow::Result<()> {
        let (view, epoch_ms) = {
            let pr = &self.panes[idx];
            if !pr.cluster_on || pr.cluster_discs.is_empty() {
                return Ok(());
            }
            (pr.view, pr.epoch_ms)
        };
        let fp = cluster_label_font_px(self.label_font_delta);
        let line_h = fp + 2.0;
        let glyph_w = fp * MONO_ADVANCE;
        let (left, top) = (view.bounds[0], view.bounds[1]);
        let (right, bottom) = (left + view.bounds[2], top + view.bounds[3]);
        let ink = color(self.readout_label);
        let pr = &mut self.panes[idx];
        let mut found = std::mem::take(&mut pr.cluster_found);
        found.clear();
        for d in pr.cluster_discs.iter().filter(|d| !d.small) {
            let t_rel = (d.time_ms - epoch_ms) as f32;
            // The marker shader's own placement: whole physical pixels from the plot's edges.
            let x = (left + (t_rel - view.view_time0) * view.time_to_px).round();
            let y = (bottom - (d.price - view.view_price0) * view.price_to_px).round();
            if x < left || x > right || y < top || y > bottom {
                continue;
            }
            let key = (d.first_ms as i64).wrapping_mul(31).wrapping_add(d.key);
            found.push((key, x / sf, y / sf, d.total(), d.radius / sf));
        }
        // Biggest first: a crowd keeps its biggest discs labelled.
        found.sort_unstable_by(|a, b| b.3.total_cmp(&a.3));
        let mut placed = std::mem::take(&mut pr.cluster_placed);
        placed.clear();
        let mut result = Ok(());
        for &(key, x, y, total, r) in &found {
            if placed.len() >= CLUSTER_LABEL_MAX {
                break;
            }
            let text = moon_chart::clusters::short_volume(total);
            let w = text.chars().count() as f32 * glyph_w;
            if !moon_chart::clusters::label_fits(r, w, line_h) {
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
            let mut slot = key.rem_euclid(CLUSTER_RUN_SLOTS) as usize;
            if pr.cluster_runs.len() <= slot {
                pr.cluster_runs
                    .resize_with(slot + 1, GpuCanvasTextRun::default);
            }
            if let Err(err) = draw_sized_text_run(
                &mut pr.cluster_runs,
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
        pr.cluster_found = found;
        pr.cluster_placed = placed;
        result
    }
}
