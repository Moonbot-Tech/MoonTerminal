//! The percent ruler's text: its two lines beside the pointer end of the measured span.
//!
//! The lines arrive formatted (`chartdx/ruler.rs`); this pass only measures and places them, on the
//! same dense plates the cursor's own values get, because the ruler sits over candles.

use super::*;
use crate::chartdx::ruler::{RulerReadout, point_px};

/// Vertical room between the two lines' plates, in logical pixels.
const LINE_GAP: f32 = 1.0;

impl RenderState {
    /// Draw the ruler's readout on pane `idx` when the ruler lives there.
    ///
    /// Args:
    ///     ctx: Text context of the frame.
    ///     idx: Pane index.
    ///     sf: Logical-to-physical factor of the text layer.
    ///     plot: The plot's `[left, top, right, bottom]` in logical pixels.
    ///     placed: The frame's label layout, which the readout pass turns into backing plates.
    pub(super) fn prepare_ruler_text(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
        idx: usize,
        sf: f32,
        plot: [f32; 4],
        placed: &mut Vec<PlacedLabel>,
    ) -> anyhow::Result<()> {
        if self.ruler.as_ref().is_none_or(|r| r.span.pane != idx) {
            return Ok(());
        }
        // Held out for the draw, which needs `&mut self` for the text runs; put back whatever the
        // draw returns, so a failed frame does not lose the ruler.
        let ruler = self.ruler.take();
        let drawn = match &ruler {
            Some(ruler) => self.draw_ruler_text(ctx, idx, sf, plot, ruler, placed),
            None => Ok(()),
        };
        self.ruler = ruler;
        drawn
    }

    fn draw_ruler_text(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
        idx: usize,
        sf: f32,
        [plot_left, plot_top, plot_right, plot_bottom]: [f32; 4],
        ruler: &RulerReadout,
        placed: &mut Vec<PlacedLabel>,
    ) -> anyhow::Result<()> {
        let span = ruler.span;
        let Some(pr) = self.panes.get(idx) else {
            return Ok(());
        };
        let Some((x1, y1)) = point_px(&pr.view, pr.epoch_ms, span.t1_ms, span.p1) else {
            return Ok(());
        };
        let (x1, y1) = (x1 / sf, y1 / sf);
        let move_color = color(if span.up() {
            self.label_positive
        } else {
            self.label_negative
        });
        let lines: Vec<(&str, Hsla)> = std::iter::once((ruler.move_line.as_str(), move_color))
            .chain(
                ruler
                    .volume_line
                    .as_deref()
                    .map(|line| (line, color(self.readout_label))),
            )
            .collect();
        let sizes: Vec<(f32, f32)> = lines
            .iter()
            .map(|(text, _)| {
                let m = self.measure_readout_text(ctx, text);
                (m.width.as_f32(), m.line_height.as_f32())
            })
            .collect();
        let block_w = sizes.iter().map(|(w, _)| *w).fold(0.0f32, f32::max);
        let step = |h: f32| h + 2.0 * READOUT_PAD_Y + LINE_GAP;
        let block_h: f32 = sizes.iter().map(|(_, h)| step(*h)).sum::<f32>() - LINE_GAP;
        // Beyond the pointer end, on the side the drag is heading: the text never sits on the span
        // it describes, and never under the crosshair the user is aiming with.
        let x = if span.t1_ms >= span.t0_ms {
            x1 + CURSOR_BADGE_DX
        } else {
            x1 - CURSOR_BADGE_DX - block_w
        };
        let y = if span.up() {
            y1 - CURSOR_BADGE_DY - block_h
        } else {
            y1 + CURSOR_BADGE_DY
        };
        let x = clamp_anchor(
            x,
            plot_left + READOUT_PAD_X,
            plot_right - block_w - READOUT_PAD_X,
        );
        let mut y = clamp_anchor(
            y,
            plot_top + READOUT_PAD_Y,
            plot_bottom - block_h - READOUT_PAD_Y,
        );
        for ((text, ink), (w, h)) in lines.into_iter().zip(sizes) {
            self.draw_readout_text(ctx, text, x, y, 0.0, 0.0, ink)?;
            placed.push(PlacedLabel {
                x,
                y,
                ax: 0.0,
                ay: 0.0,
                w,
                h,
                solid: true,
            });
            y += step(h);
        }
        Ok(())
    }
}
