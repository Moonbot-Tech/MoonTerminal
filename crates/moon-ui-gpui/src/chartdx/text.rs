//! Text emitted by chart `gpu_canvas.prepare_text`: axis labels and cursor readout.
//! This keeps chart-zone text on the retained GPU path instead of repainting the
//! GPUI view tree on every mouse move.

use gpui::{GpuCanvasTextMetrics, Hsla, point, px};
use moon_chart::axes::{fmt_clock, nice_interval, price_decimals};

use super::*;

const FONT_SIZE: f32 = 11.5;
pub(super) const LINE_H: f32 = FONT_SIZE + 4.0;
const READOUT_PAD_X: f32 = 5.0;
const READOUT_PAD_Y: f32 = 2.5;
const READOUT_INSET: f32 = 2.0;
// Угловая подпись (имя ядра + тикер). Якорь правым краем: есть стакан → у края панели (над
// стаканом, слева от ✕ закрытия); нет стакана → у края плота (в области графика). Инсет 20px
// освобождает крайние ~18px под ✕. pub(super): render_state строит по ним прозрачную плашку.
pub(super) const CAPTION_PAD_X: f32 = 20.0;
pub(super) const CAPTION_PAD_Y: f32 = 4.0;
const FIRETEST_TEXT_FONT_SIZE: f32 = 9.0;
const FIRETEST_TEXT_LINE_H: f32 = 11.0;

fn color(hex: u32) -> Hsla {
    gpui::rgb(hex).into()
}

fn mix_hex(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |shift| {
        let av = ((a >> shift) & 0xff_u32) as f32;
        let bv = ((b >> shift) & 0xff_u32) as f32;
        (av + (bv - av) * t).round() as u32
    };
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

fn local_offset_sec() -> i64 {
    crate::axes::local_offset_sec()
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

fn clamp_anchor(value: f32, min: f32, max: f32) -> f32 {
    if min <= max {
        value.clamp(min, max)
    } else {
        (min + max) * 0.5
    }
}

impl RenderState {
    pub(super) fn set_firetest_text_labels(&mut self, count: usize) -> bool {
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

    fn draw_text(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
        text: &str,
        x: f32,
        y: f32,
        ax: f32,
        ay: f32,
        color: Hsla,
    ) -> anyhow::Result<GpuCanvasTextMetrics> {
        if self.text_run_cursor >= self.text_runs.len() {
            self.text_runs.push(GpuCanvasTextRun::default());
        }
        let run = &mut self.text_runs[self.text_run_cursor];
        self.text_run_cursor += 1;
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

    fn measure_text(&mut self, ctx: &GpuCanvasTextContext<'_>, text: &str) -> GpuCanvasTextMetrics {
        if self.text_run_cursor >= self.text_runs.len() {
            self.text_runs.push(GpuCanvasTextRun::default());
        }
        self.text_runs[self.text_run_cursor].measure(
            ctx,
            text,
            gpui::font(crate::design::mono()),
            px(FONT_SIZE),
            px(LINE_H),
        )
    }

    fn draw_firetest_text(
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

    pub(super) fn prepare_text(
        &mut self,
        ctx: &mut GpuCanvasTextContext<'_>,
    ) -> anyhow::Result<()> {
        self.text_run_cursor = 0;
        let sf = ctx.scale_factor().max(0.1);
        let palette = self.ui_palette;
        let ink = color(palette.text_soft);
        let readout = color(mix_hex(palette.text_soft, palette.text, 0.45));
        // Угловая подпись — светлым шрифтом (самый яркий текст палитры), без подложки.
        let caption_fg = color(palette.text);
        let tz_offset_sec = local_offset_sec();
        let mut firetest_text_drawn = false;
        let mut readout_metrics_changed = false;

        for idx in 0..self.panes.len() {
            let (
                active,
                pane_bounds,
                view,
                epoch_ms,
                core_name,
                market,
                orderbook_enabled,
                price_axis_pos,
                time_axis_visible,
            ) = {
                let pr = &self.panes[idx];
                (
                    pr.active,
                    pr.pane_bounds,
                    pr.view,
                    pr.epoch_ms,
                    pr.core_name.clone(),
                    pr.market.clone(),
                    pr.orderbook_enabled,
                    pr.price_axis_pos,
                    pr.time_axis_visible,
                )
            };
            if !active {
                continue;
            }
            let pane_left = pane_bounds[0] / sf;
            let pane_right = (pane_bounds[0] + pane_bounds[2]) / sf;
            let pane_bottom = (pane_bounds[1] + pane_bounds[3]) / sf;
            let plot_left = view.bounds[0] / sf;
            let plot_top = view.bounds[1] / sf;
            let plot_w = view.bounds[2] / sf;
            let plot_h = view.bounds[3] / sf;
            let plot_bottom = plot_top + plot_h;
            let plot_right = plot_left + plot_w;
            // Сторона оси цен: Left → подписи в жёлобе слева от плота; Right → справа у края панели
            // (жёлоб за стаканом); Hide → ось не рисуем вовсе. Правый якорь текста (align 1.0) общий.
            use crate::chart_persist::PriceAxisPos;
            let axis_hidden = matches!(price_axis_pos, PriceAxisPos::Hide);
            let axis_on_right = matches!(price_axis_pos, PriceAxisPos::Right);
            let axis_label_x = if axis_on_right {
                pane_right - 4.0
            } else {
                plot_left - 4.0
            };

            // Угловая подпись: имя ядра + тикер, светлый текст на прозрачной плашке (её строит
            // render_state по `caption_w`). Якорь правым краем: есть стакан → у края панели (над
            // стаканом), нет стакана → у края плота (в области графика). Тот же выбор повторён в
            // render_state для плашки — держать синхронно. Рисуем ДО гейта по `plot_w`, чтобы в
            // режиме «только стакан» (чарт схлопнут) подпись осталась над стаканом.
            {
                let right_edge = if orderbook_enabled {
                    pane_right
                } else {
                    plot_right
                };
                let cap_x = right_edge - CAPTION_PAD_X;
                let cap_y = plot_top + CAPTION_PAD_Y;
                let mut cap_w = 0.0_f32;
                if !core_name.is_empty() {
                    cap_w = cap_w.max(self.measure_text(ctx, &core_name).width.as_f32());
                    self.draw_text(ctx, &core_name, cap_x, cap_y, 1.0, 0.0, caption_fg)?;
                }
                let ticker = moon_core::symbol::display_pair(&market);
                if !ticker.is_empty() {
                    cap_w = cap_w.max(self.measure_text(ctx, &ticker).width.as_f32());
                    self.draw_text(ctx, &ticker, cap_x, cap_y + LINE_H, 1.0, 0.0, caption_fg)?;
                }
                if (self.panes[idx].caption_w - cap_w).abs() > 0.25 {
                    self.panes[idx].caption_w = cap_w;
                    readout_metrics_changed = true;
                }
            }

            // Дальше — оси/курсор/сетка, только для нормального (не схлопнутого) чарта.
            if plot_w < 60.0 || plot_h < 60.0 || view.price_to_px <= 0.0 {
                continue;
            }

            if !firetest_text_drawn {
                self.draw_firetest_text(ctx, plot_left, plot_top, plot_w, plot_h, ink)?;
                firetest_text_drawn = true;
            }

            let price_to_px = view.price_to_px / sf;
            let price_range = plot_h / price_to_px.max(1e-6);
            let y_min = view.view_price0;
            let top_price = y_min + price_range;
            let interval = nice_interval(price_range.max(1e-9), 8.0);
            let dec = price_decimals(y_min + price_range * 0.5);
            let time_to_px = (view.time_to_px / sf).max(1e-6);
            let window_ms = plot_w as f64 / time_to_px as f64;
            let left_unix = epoch_ms + view.view_time0 as f64;

            let cursor = self.cursor.filter(|cursor| cursor.pane == idx);
            let mut skip_time_label_x = None;
            let mut skip_price_label_y = None;

            if let Some(cursor) = cursor {
                let cx_log = (self.slot_origin[0] + cursor.local[0]) / sf;
                let cy_log = (self.slot_origin[1] + cursor.local[1]) / sf;

                if cx_log >= plot_left && cx_log <= plot_right {
                    let unix = left_unix + (cx_log - plot_left) as f64 / time_to_px as f64;
                    let label = fmt_clock(unix, tz_offset_sec, true);
                    let metrics = self.measure_text(ctx, &label);
                    let width = metrics.width.as_f32();
                    if (self.panes[idx].readout_time_width - width).abs() > 0.25 {
                        self.panes[idx].readout_time_width = width;
                        readout_metrics_changed = true;
                    }
                    let half_w = metrics.width.as_f32() * 0.5;
                    let x = clamp_anchor(
                        cx_log,
                        plot_left + half_w + READOUT_PAD_X + READOUT_INSET,
                        plot_right - half_w - READOUT_PAD_X - READOUT_INSET,
                    );
                    let y = pane_bottom - 1.0;
                    let dst = readout_rect_dst(x, y, metrics, 0.5, 1.0, sf);
                    self.draw_text(ctx, &label, x, y, 0.5, 1.0, readout)?;
                    skip_time_label_x = Some(rect_x_range_log(dst, sf));
                }

                if !axis_hidden && cy_log >= plot_top && cy_log <= plot_bottom {
                    let price = y_min + (plot_bottom - cy_log) / price_to_px.max(1e-6);
                    let label = format!("{price:.dec$}");
                    let metrics = self.measure_text(ctx, &label);
                    let width = metrics.width.as_f32();
                    if (self.panes[idx].readout_price_width - width).abs() > 0.25 {
                        self.panes[idx].readout_price_width = width;
                        readout_metrics_changed = true;
                    }
                    // Right → плашка у правого края панели (за стаканом); Left → у левого жёлоба.
                    let x = if axis_on_right {
                        pane_right - 3.0
                    } else {
                        (plot_left - 3.0)
                            .max(pane_left + READOUT_INSET + READOUT_PAD_X + metrics.width.as_f32())
                    };
                    let dst = readout_rect_dst(x, cy_log, metrics, 1.0, 0.5, sf);
                    self.draw_text(ctx, &label, x, cy_log, 1.0, 0.5, readout)?;
                    skip_price_label_y = Some(rect_y_range_log(dst, sf));
                }
            }

            // Прореживание по вертикали: при низком окне «nice»-шаг даёт подписи плотнее строки —
            // рисуем следующую, только если она отстоит от ПРЕДЫДУЩЕЙ нарисованной на высоту
            // строки (иначе пропуск → «через одну»). last_y идёт сверху вниз (p растёт → y ↓).
            let min_v_gap = LINE_H;
            let mut last_y = f32::INFINITY;
            let mut p = (y_min / interval).ceil() * interval;
            let mut guard = 0;
            while !axis_hidden && p <= top_price && guard < 256 {
                let y = plot_bottom - (p - y_min) * price_to_px;
                let overlaps_readout = skip_price_label_y
                    .is_some_and(|(top, bottom)| y >= top - 1.0 && y <= bottom + 1.0);
                if y >= plot_top - 1.0
                    && y <= plot_bottom + 1.0
                    && !overlaps_readout
                    && (last_y - y).abs() >= min_v_gap
                {
                    let label = format!("{p:.dec$}");
                    self.draw_text(ctx, &label, axis_label_x, y, 1.0, 0.5, ink)?;
                    last_y = y;
                }
                p += interval;
                guard += 1;
            }

            let div_sec = window_ms / 1000.0 / 6.0;
            let with_sec = div_sec < 60.0;
            // Прореживание по горизонтали: при узком окне 7 подписей налезают друг на друга —
            // рисуем подпись, только если её левый край отстоит от ПРАВОГО края предыдущей
            // нарисованной (иначе пропуск → «через одну»).
            let min_h_gap = 6.0;
            let mut last_right = f32::NEG_INFINITY;
            for k in 0..=6 {
                if !time_axis_visible {
                    break;
                }
                let frac = k as f64 / 6.0;
                let x = plot_left + (frac as f32) * plot_w;
                let unix = left_unix + frac * window_ms;
                let label = fmt_clock(unix, tz_offset_sec, with_sec);
                let metrics = self.measure_text(ctx, &label);
                let half_w = metrics.width.as_f32() * 0.5;
                let left = x - half_w;
                let right = x + half_w;
                let overlaps_readout = skip_time_label_x.is_some_and(|(skip_left, skip_right)| {
                    right >= skip_left && left <= skip_right
                });
                if !overlaps_readout && left >= last_right + min_h_gap {
                    self.draw_text(ctx, &label, x, pane_bottom - 2.0, 0.5, 1.0, ink)?;
                    last_right = right;
                }
            }
        }

        if readout_metrics_changed {
            self.sync_readout_params();
            self.needs_present = true;
        }

        if self.text_run_cursor < self.text_runs.len() {
            for run in &mut self.text_runs[self.text_run_cursor..] {
                run.clear();
            }
        }
        Ok(())
    }
}
