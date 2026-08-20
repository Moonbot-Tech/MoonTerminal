//! `RenderState` draw/measure helpers, FireTest text, and ghost-crosshair labels.

use gpui::{GpuCanvasTextMetrics, Hsla, point, px};

use super::*;

impl RenderState {
    pub(crate) fn set_firetest_text_labels(&mut self, count: usize) -> bool {
        if self.firetest_text_labels.len() == count {
            return false;
        }
        self.firetest_text_labels.clear();
        self.firetest_text_labels.reserve(count);
        for i in 0..count {
            self.firetest_text_labels
                .push(format!("This is a Line {i:04} \u{203C}\u{FE0F}"));
        }
        self.firetest_text_runs
            .resize_with(count, GpuCanvasTextRun::default);
        self.firetest_text_runs.truncate(count);
        self.firetest_text_layer.clear();
        self.firetest_text_revision = self.firetest_text_revision.wrapping_add(1);
        self.needs_present = true;
        true
    }

    pub(super) fn draw_text(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
        text: &str,
        x: f32,
        y: f32,
        ax: f32,
        ay: f32,
        color: Hsla,
    ) -> anyhow::Result<GpuCanvasTextMetrics> {
        draw_text_run(
            &mut self.text_runs,
            &mut self.text_run_cursor,
            ctx,
            text,
            x,
            y,
            ax,
            ay,
            color,
        )
    }

    pub(super) fn measure_text(
        &mut self,
        ctx: &GpuCanvasTextContext<'_>,
        text: &str,
    ) -> GpuCanvasTextMetrics {
        measure_text_run(&mut self.text_runs, self.text_run_cursor, ctx, text)
    }

    /// Draws text with a custom font size for the large anchor delta in broom mode.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_sized_text(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
        text: &str,
        size: f32,
        x: f32,
        y: f32,
        ax: f32,
        ay: f32,
        color: Hsla,
    ) -> anyhow::Result<GpuCanvasTextMetrics> {
        draw_sized_text_run(
            &mut self.text_runs,
            &mut self.text_run_cursor,
            ctx,
            text,
            size,
            x,
            y,
            ax,
            ay,
            color,
        )
    }

    /// Returns the order-line and cursor label size: `FONT_SIZE` plus the theme setting.
    ///
    /// Clamps the settings-slider adjustment to safe bounds so it cannot break layout.
    pub(super) fn label_font_px(&self) -> f32 {
        label_font_px(self.label_font_delta)
    }

    /// Draws text with the order-line label size from `label_font_px` and a `size + 4` line height.
    pub(super) fn draw_label_text(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
        text: &str,
        x: f32,
        y: f32,
        ax: f32,
        ay: f32,
        color: Hsla,
    ) -> anyhow::Result<GpuCanvasTextMetrics> {
        draw_label_text_run(
            &mut self.text_runs,
            &mut self.text_run_cursor,
            ctx,
            self.label_font_delta,
            text,
            x,
            y,
            ax,
            ay,
            color,
        )
    }

    /// Measures text with the order-line label size from `label_font_px`.
    pub(super) fn measure_label_text(
        &mut self,
        ctx: &GpuCanvasTextContext<'_>,
        text: &str,
    ) -> GpuCanvasTextMetrics {
        measure_label_text_run(
            &mut self.text_runs,
            self.text_run_cursor,
            ctx,
            self.label_font_delta,
            text,
        )
    }

    pub(super) fn draw_firetest_text(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
        plot_left: f32,
        plot_top: f32,
        plot_w: f32,
        plot_h: f32,
        color: Hsla,
    ) -> anyhow::Result<()> {
        let count = self.firetest_text_labels.len();
        if count == 0 {
            return Ok(());
        }

        // FireTest intentionally bakes the whole retained set, but draws only a
        // physically visible page. Drawing all 10k labels every present would
        // measure GPU fill/instance cost, not retained text churn.
        let cols = ((plot_w / 150.0).floor() as usize).clamp(1, count);
        let rows = ((plot_h / (FIRETEST_TEXT_LINE_H + 4.0)).floor() as usize)
            .max(1)
            .min(count.div_ceil(cols));
        let visible_count = count.min(cols.saturating_mul(rows).max(1));
        let step_x = plot_w / cols as f32;
        let step_y = plot_h / rows as f32;
        let font = gpui::font(crate::design::mono());
        let layout_key = (count as u64)
            ^ ((visible_count as u64) << 3)
            ^ ((cols as u64) << 17)
            ^ ((rows as u64) << 29)
            ^ ((step_x.to_bits() as u64) << 7)
            ^ ((step_y.to_bits() as u64) << 39);
        let mut drawn = 0_u64;
        let mut cold = 0_u64;
        ctx.draw_retained_text_layer(
            &mut self.firetest_text_layer,
            layout_key,
            self.firetest_text_revision,
            GpuCanvasTextTransform::identity(),
            0..visible_count as u32,
            |builder| {
                for i in 0..count {
                    let page = i / visible_count;
                    let local = i % visible_count;
                    let col = local % cols;
                    let row = local / cols;
                    let x = plot_left + page as f32 * (plot_w + step_x) + col as f32 * step_x;
                    let y = plot_top + row as f32 * step_y;
                    let run = &mut self.firetest_text_runs[i];
                    if !run.is_cached() {
                        cold += 1;
                    }
                    builder.set_label_id(i as u32);
                    run.draw(
                        builder.context(),
                        point(px(x), px(y)),
                        self.firetest_text_labels[i].as_str(),
                        font.clone(),
                        px(FIRETEST_TEXT_FONT_SIZE),
                        px(FIRETEST_TEXT_LINE_H),
                        color,
                    )?;
                    drawn += 1;
                }
                Ok(())
            },
        )?;

        crate::diag::bump_by(&crate::diag::FIRETEST_TEXT_DRAW, drawn);
        crate::diag::bump_by(&crate::diag::FIRETEST_TEXT_COLD, cold);
        Ok(())
    }

    /// Draws compare-mode ghost-crosshair labels for a pane without a real cursor.
    ///
    /// At `ghost_price`, draws only order-book volume at the level (above the line) and the
    /// percentage from the same book reference the real cursor uses (below it), without
    /// duplicating time, axis price, or order size. Also works in a collapsed broom chart by
    /// mapping price to Y through the order-book view, whose height and price window match the
    /// collapsed chart. The backend cursor layer draws the line; see `sync_cursor_params`, which
    /// places the ghost's X outside the bounds.
    pub(super) fn draw_ghost_cursor_labels(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
        idx: usize,
        sf: f32,
        placed: &mut Vec<PlacedLabel>,
    ) -> anyhow::Result<()> {
        let Some(price) = self.ghost_price else {
            return Ok(());
        };
        if !self.cursor_labels || self.cursor.is_some_and(|c| c.pane == idx) {
            return Ok(());
        }
        let (view, orderbook_view, pane_bounds, orderbook_enabled, cached_last_price, book_best) = {
            let pr = &self.panes[idx];
            (
                pr.view,
                pr.orderbook_view,
                pr.pane_bounds,
                pr.orderbook_enabled,
                pr.cached_last_price,
                pr.book_best,
            )
        };
        // Use the same "normal" chart threshold as the main cursor block (plot_w >= 60).
        let v = if view.price_to_px > 0.0 && view.bounds[2] / sf >= 60.0 {
            view
        } else {
            orderbook_view
        };
        if v.price_to_px <= 0.0 {
            return Ok(());
        }
        let price_to_px = v.price_to_px / sf;
        let top = v.bounds[1] / sf;
        let bottom = top + v.bounds[3] / sf;
        let cy = bottom - (price - v.view_price0) * price_to_px;
        if !(cy >= top && cy <= bottom) {
            return Ok(());
        }
        // Anchor X like the real cursor: to the right of the separator (the order book's left
        // edge; in broom mode the full-width book starts at the panel's left edge). Without an
        // order book, use the control zone's left edge.
        let pane_left = pane_bounds[0] / sf;
        let pane_right = (pane_bounds[0] + pane_bounds[2]) / sf;
        // One home for "where does the order book's zone start"; see `text::caption`. The cursor
        // needs only that edge, so it calls the primitive rather than the caption's own geometry.
        let zone_left = super::book_zone_left(
            pane_left,
            pane_right,
            pane_left,
            pane_right,
            orderbook_enabled,
            orderbook_view.bounds[0] / sf,
        );
        let right_x = zone_left + READOUT_PAD_X;
        // Keep the label badge from cutting through the horizontal line; see cursor_label_gap.
        let gap = cursor_label_gap(self.cursor_thickness, sf);
        // Same reference as the real cursor: the nearest side of the book, so one price reads the
        // same percentage whether the pointer is on this chart or ghosted from its compare peer.
        let ghost_ref = cached_last_price
            .filter(|l| *l > 0.0)
            .map(|last| cursor_ref_price(book_best, last, price));
        let cur_col = ghost_ref
            .map(|r| pct_hsla(r - price, self.label_positive, self.label_negative))
            .unwrap_or(color(self.readout_label));
        // Draw order-book volume at the ghost level above the line.
        if orderbook_enabled && !self.panes[idx].orderbook_levels.is_empty() {
            let tol = 6.0 / price_to_px.max(1e-6);
            if let Some(q) =
                nearest_orderbook_notional(&self.panes[idx].orderbook_levels, price, tol)
            {
                let m = self.draw_label_text(
                    ctx,
                    &fmt_amount(q),
                    right_x,
                    cy - gap,
                    0.0,
                    1.0,
                    cur_col,
                )?;
                placed.push(PlacedLabel {
                    x: right_x,
                    y: cy - gap,
                    ax: 0.0,
                    ay: 1.0,
                    w: m.width.as_f32(),
                    h: m.line_height.as_f32(),
                    solid: true,
                });
            }
        }
        // Draw the ghost's percentage deviation from this chart's book reference below the line.
        // `ghost_ref` is Some only for a positive last price, and the book side it may return in
        // its place is positive too, so the division needs no further guard.
        if let Some(r) = ghost_ref {
            let pct = (price - r) / r * 100.0;
            let m =
                self.draw_label_text(ctx, &fmt_pct(pct), right_x, cy + gap, 0.0, 0.0, cur_col)?;
            placed.push(PlacedLabel {
                x: right_x,
                y: cy + gap,
                ax: 0.0,
                ay: 0.0,
                w: m.width.as_f32(),
                h: m.line_height.as_f32(),
                solid: true,
            });
        }
        Ok(())
    }
}
