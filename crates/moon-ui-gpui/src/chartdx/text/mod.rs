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
// Отступ подписи ордер-линии от самой линии (px). Достаточный, чтобы плашка (её низ/верх =
// dy ± READOUT_PAD_Y) не накрывала линию: GAP > READOUT_PAD_Y.
const LABEL_LINE_GAP: f32 = 4.0;
// Угловая подпись (имя ядра + тикер). Якорь правым краем: есть стакан → у края панели (над
// стаканом, слева от ✕ закрытия); нет стакана → у края плота (в области графика). Кнопка ✕
// занимает крайние ~26px (bounds `right-26`, ширина 22), поэтому инсет 30px уводит правый край
// подписи ЛЕВЕЕ кнопки (зазор ~4px), чтобы текст не прятался под ней. pub(super): render_state
// строит по ним прозрачную плашку (тем же инсетом — едет вместе с текстом).
pub(super) const CAPTION_PAD_X: f32 = 30.0;
pub(super) const CAPTION_PAD_Y: f32 = 4.0;
// Зазор между бейджем текущего Y-масштаба и блоком угловой подписи (бейдж — левее).
pub(super) const CAPTION_SCALE_GAP: f32 = 8.0;
const FIRETEST_TEXT_FONT_SIZE: f32 = 9.0;
const FIRETEST_TEXT_LINE_H: f32 = 11.0;

fn color(hex: u32) -> Hsla {
    gpui::rgb(hex).into()
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

/// Отступ курсорных подписей (размер/объём/%) от горизонтали перекрестия: плашка подписи
/// (текст ± `READOUT_PAD_Y`) не должна накрывать саму линию — иначе видимый «разрыв» курсора.
/// Учитывает толщину креста (device px → лог.) с запасом 1px.
fn cursor_label_gap(cursor_thickness_dev: f32, sf: f32) -> f32 {
    LABEL_LINE_GAP.max(READOUT_PAD_Y + cursor_thickness_dev / sf.max(0.1) * 0.5 + 1.0)
}

/// «+1.25%» — знаковый процент для подписей курсора (отклонение от текущей цены).
/// Общий и для подписей ордер-линий (`data_state::orders`).
///
/// Deliberately NOT `moon_core::util::fmt::signed_pct`: this runs per label in the GPU frame
/// path on `f32` cursor deltas, and a deviation that rounds to zero sits at the cursor's own
/// price line, where the sign carries the direction the reader is dragging toward — the ambiguity
/// the shared formatter removes is information here.
pub(in crate::chartdx) fn fmt_pct(v: f32) -> String {
    format!("{v:+.2}%")
}

/// Компактное накопленное количество стакана с SI-суффиксом K/M/B/T — для подписи курсора.
fn fmt_amount(v: f32) -> String {
    moon_core::util::fmt::compact_si(v as f64)
}

/// Цвет знакового процента: плюс → positive, минус → negative из текущей chart theme.
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

/// `draw_text_run` с произвольным кеглем (высота строки = кегль+4) — крупная дельта от якоря
/// в метле. Кегль передаётся снаружи (масштабируется слайдером через `label_font_px`).
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

// Разнос по смысловым блокам (verbatim-перенос): runs — draw/measure-обёртки RenderState,
// firetest и призрак курсора; prepare — главный prepare_text.
mod prepare;
mod runs;
