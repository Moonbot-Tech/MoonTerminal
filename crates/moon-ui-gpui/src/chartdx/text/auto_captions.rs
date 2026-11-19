//! The captions on auto lines: placed here, built elsewhere.
//!
//! The strings are made on revisions by `auto_figures_sync::pane::refresh_auto_captions` — a
//! rebuild of the lines, a price change that moved a printed digit, the age clock — so this pass,
//! which runs on every presented frame, does arithmetic only: where the line's drawn stretch is on
//! screen, the line's price there, and which side of it the text goes.

use moon_core::config::auto_figures::CaptionPlace;

use super::*;

impl RenderState {
    /// Draw pane `idx`'s auto-line captions.
    ///
    /// An `OnHover` caption is drawn only for the line under the pointer (`auto_hover`). A line
    /// whose drawn stretch is off screen, or whose price at the anchor is outside the plot, prints
    /// nothing.
    ///
    /// Args:
    ///     ctx: Text context of the frame.
    ///     idx: Pane index.
    ///     sf: Logical-to-physical factor of the text layer.
    ///     plot: The plot's `[left, top, right, bottom]` in logical pixels.
    ///     label_x: The right edge of the price-label column, where `AtPriceAxis` captions end.
    ///     placed: The frame's label layout.
    pub(super) fn draw_auto_captions(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
        idx: usize,
        sf: f32,
        [plot_left, plot_top, plot_right, plot_bottom]: [f32; 4],
        label_x: f32,
        placed: &mut Vec<PlacedLabel>,
    ) -> anyhow::Result<()> {
        let Some(pr) = self.panes.get(idx) else {
            return Ok(());
        };
        if pr.auto.lines.is_empty() {
            return Ok(());
        }
        let view = pr.view;
        let epoch_ms = pr.epoch_ms;
        let hovered = self
            .auto_hover
            .filter(|(pane, _)| *pane == idx)
            .map(|(_, key)| key);
        let time_to_px = (view.time_to_px / sf).max(moon_chart::view::MIN_PX_PER_MS);
        let left_unix = epoch_ms + f64::from(view.view_time0);
        let x_of =
            |t_ms: f64| plot_left + ((t_ms - epoch_ms) as f32 - view.view_time0) * time_to_px;
        let t_at = |x: f32| left_unix + f64::from((x - plot_left) / time_to_px);
        let line_y = |price: f32| -> f32 {
            ((plot_bottom * sf) - (price - view.view_price0) * view.price_to_px).round() / sf
        };
        for li in 0..self.panes[idx].auto.lines.len() {
            // Copied out: the text is an `Arc<str>` (a refcount bump), and drawing needs `&mut self`.
            let (text, shape, span, place, rgb) = {
                let line = &self.panes[idx].auto.lines[li];
                let Some(text) = line.text.clone() else {
                    continue;
                };
                if line.hover_only() && hovered != Some(line.key) {
                    continue;
                }
                (text, line.shape, line.span, line.place(), line.color)
            };
            // The part of the drawn stretch that is on screen.
            let x_lo = if span.0.is_finite() {
                x_of(span.0).max(plot_left)
            } else {
                plot_left
            };
            let x_hi = if span.1.is_finite() {
                x_of(span.1).min(plot_right)
            } else {
                plot_right
            };
            if x_hi <= x_lo {
                continue;
            }
            // (text anchor x, horizontal alignment, where on the line the price is read).
            let (x, ax, at_x) = match place {
                CaptionPlace::LeftEnd => (x_lo + READOUT_PAD_X, 0.0, x_lo),
                CaptionPlace::RightEnd => (x_hi - READOUT_PAD_X, 1.0, x_hi),
                // The label column the order labels use, read where the line meets the plot edge.
                CaptionPlace::AtPriceAxis => (label_x, 1.0, x_hi),
                CaptionPlace::AboveLine | CaptionPlace::BelowLine => {
                    let mid = (x_lo + x_hi) * 0.5;
                    (mid, 0.5, mid)
                }
            };
            let Some(price) = shape.price_at(t_at(at_x)) else {
                continue;
            };
            let y = line_y(price as f32);
            if !(plot_top..=plot_bottom).contains(&y) {
                continue;
            }
            let (dy, ay) = match place {
                CaptionPlace::BelowLine => (y + LABEL_LINE_GAP, 0.0),
                _ => (y - LABEL_LINE_GAP, 1.0),
            };
            let m = draw_label_text_run(
                &mut self.text_runs,
                &mut self.text_run_cursor,
                ctx,
                self.label_font_delta,
                &text,
                x,
                dy,
                ax,
                ay,
                color(rgb),
            )?;
            placed.push(PlacedLabel {
                x,
                y: dy,
                ax,
                ay,
                w: m.width.as_f32(),
                h: m.line_height.as_f32(),
                solid: false,
            });
        }
        Ok(())
    }
}
