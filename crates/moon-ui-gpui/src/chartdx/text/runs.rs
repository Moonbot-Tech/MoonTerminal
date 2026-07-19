//! Draw/measure-обёртки `RenderState`, firetest-текст и призрак перекрестия
//! (вынос из text.rs, verbatim).

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

    /// `draw_text` с произвольным кеглем — крупная дельта от якоря в метле.
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

    /// Кегль подписей ордер-линий и курсора = база `FONT_SIZE` + поправка из темы (слайдер
    /// Настроек). Зажат в разумные границы, чтобы не сломать раскладку.
    pub(super) fn label_font_px(&self) -> f32 {
        label_font_px(self.label_font_delta)
    }

    /// `draw_text`, но кеглем подписей ордер-линий (`label_font_px`). Высота строки = кегль+4.
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

    /// `measure_text`, но кеглем подписей ордер-линий (`label_font_px`).
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

    /// Призрак перекрестия compare-режима: у панели БЕЗ реального курсора рисуем на цене
    /// `ghost_price` ТОЛЬКО объём стакана на уровне (над линией) и % от текущей цены (под
    /// линией) — время/цену-на-оси/размер ордера не дублируем (решение пользователя).
    /// Работает и на схлопнутом чарте метлы: маппинг цена→Y берём из вида стакана, когда
    /// чарт-вид схлопнут (высота и ценовое окно у них совпадают). Сама линия — cursor.hlsl
    /// (см. `sync_cursor_params`, призрак с X за границами).
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
        let (view, orderbook_view, pane_bounds, orderbook_enabled, cached_last_price) = {
            let pr = &self.panes[idx];
            (
                pr.view,
                pr.orderbook_view,
                pr.pane_bounds,
                pr.orderbook_enabled,
                pr.cached_last_price,
            )
        };
        // Тот же порог «нормального» чарта, что и у основного курсорного блока (plot_w>=60).
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
        // Якорь X — как у реального курсора: правее разделителя (левый край стакана; в метле
        // стакан на всю ширину → левый край панели). Без стакана — левый край зоны управления.
        let pane_left = pane_bounds[0] / sf;
        let pane_right = (pane_bounds[0] + pane_bounds[2]) / sf;
        let zone_left = if orderbook_enabled {
            orderbook_view.bounds[0] / sf
        } else {
            let zone_w = moon_chart::GLASS_ZONE_PX.min((pane_right - pane_left) * 0.5);
            pane_right - zone_w
        };
        let right_x = zone_left + READOUT_PAD_X;
        // Зазор от линии: плашка подписи не должна резать горизонталь (см. cursor_label_gap).
        let gap = cursor_label_gap(self.cursor_thickness, sf);
        let cur_col = cached_last_price
            .filter(|l| *l > 0.0)
            .map(|last| pct_hsla(last - price, self.label_positive, self.label_negative))
            .unwrap_or(color(self.readout_label));
        // Объём стакана на уровне призрака — над линией.
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
        // % отклонения призрака от ТЕКУЩЕЙ цены этого чарта — под линией.
        if let Some(last) = cached_last_price {
            if last > 0.0 {
                let pct = (price - last) / last * 100.0;
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
        }
        Ok(())
    }
}
