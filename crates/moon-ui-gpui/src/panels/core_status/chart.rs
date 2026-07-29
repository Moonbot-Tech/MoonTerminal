//! Detached-window CPU / memory time chart for one server.
//!
//! Built only when the panel is a full window (never a dock tab, so the tab pays no canvas cost).
//! The server contributes two filled-and-stroked series on a fixed 0..100 % Y axis: machine CPU and
//! occupied memory. When the server runs more than one core, each core adds a thinner, unfilled pair
//! (process CPU strong, process memory faint) in its own hue, drawn beneath the server pair. The X
//! axis is the FULL selected window (5 min or 1 hour) and is always drawn at full width — data grows
//! in from the right instead of stretching. Left: % scale. Right: current server values. Records
//! continuously, unlike the snapshot table.

use std::collections::VecDeque;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::{ChartWindow, CoreStatusView};
use crate::design;

/// Plot height in pixels (the drawing area between legend and X labels).
const PLOT_H: f32 = 110.0;
/// Left gutter for the percent scale.
const AXIS_W: f32 = 26.0;
/// Right gutter for the current-value labels.
const VAL_W: f32 = 44.0;
/// Horizontal grid lines on the percent axis.
const GRID: [u8; 5] = [0, 25, 50, 75, 100];
/// Vertical grid divisions across the whole window (same count for 5 min and 1 hour, so 5 min → a
/// line every 10 s, 1 hour → every 2 min).
const GRID_DIVISIONS: usize = 30;
/// A HH:MM:SS time label is placed at every Nth vertical division.
const LABEL_EVERY: usize = 6;
/// Stroke width of the two prominent server series.
const SERVER_STROKE: f32 = 1.5;
/// Stroke width of the thinner per-core series.
const CORE_STROKE: f32 = 1.0;
/// Alpha of a core's fainter (memory) line, relative to its solid (CPU) line.
const CORE_MEM_ALPHA: f32 = 0.5;

/// One core's process history and its assigned series hue, overlaid on the server chart.
pub(super) struct CoreLine {
    /// Core display name shown in the wrapped core legend.
    pub(super) name: String,
    /// `(process CPU %, process memory share %)` samples, oldest first.
    pub(super) points: VecDeque<(u8, u8)>,
    /// Palette color this core's pair is drawn in.
    pub(super) color: u32,
}

/// Fallback per-core series hue, cycled from the palette and deliberately skipping the server's
/// blue/green AND the ping line's accent so a core line never reads as a server or ping line.
///
/// Args:
///     p: Active Moon palette.
///     i: Core index within its server.
///
/// Returns:
///     A palette color for the core's line pair.
pub(super) fn core_line_color(p: MoonPalette, i: usize) -> u32 {
    [
        p.orange,
        p.amber,
        p.red,
        p.yellow,
        p.text_muted,
        p.text_soft,
    ][i % 6]
}

/// One core's prepared draw data: both metric series in plot-fraction space plus its two line hues.
struct CoreDraw {
    /// Process-CPU series as `(x fraction, value %)`.
    cpu: Vec<(f32, f32)>,
    /// Process-memory-share series as `(x fraction, value %)`.
    mem: Vec<(f32, f32)>,
    /// Solid CPU line color.
    line: Hsla,
    /// Fainter memory line color.
    faint: Hsla,
}

/// Map a ring to `(x fraction, value)` points for one metric, anchoring the newest sample to the
/// right edge so shorter rings grow in from the right and stay time-aligned with longer ones.
///
/// Args:
///     points: History samples, oldest first (a `(cpu, mem)` pair or a scalar ping `u16`).
///     span: Number of seconds the window covers.
///     pick: Selector turning one sample into the plotted value.
///
/// Returns:
///     Up to `span` points with x in `0.0..=1.0`.
fn to_series<T>(points: &VecDeque<T>, span: usize, pick: impl Fn(&T) -> f32) -> Vec<(f32, f32)> {
    let len = points.len();
    let show = len.min(span);
    let start = len - show;
    let denom = span.saturating_sub(1).max(1) as f32;
    (0..show)
        .map(|j| {
            let x = 1.0 - ((show - 1 - j) as f32) / denom;
            (x, pick(&points[start + j]))
        })
        .collect()
}

/// Render one server's CPU/memory history — the server pair plus any per-core pairs — as a chart
/// with axes and window buttons.
///
/// Args:
///     server_points: `(cpu %, occupied memory %)` machine samples, oldest first.
///     core_series: Per-core process histories to overlay (empty for a single-core server).
///     title: Server display name shown in the legend.
///     window: Selected time span (5 min or 1 hour).
///     now_sec: Current Unix second for the X-axis labels.
///     view: Panel handle for the window-switch buttons.
///     p: Active Moon palette.
///     cx: Application context for themed text sizing.
///
/// Returns:
///     A fixed-height chart block for the bottom of the detached window.
#[allow(clippy::too_many_arguments)]
pub(super) fn server_chart(
    server_points: &VecDeque<(u8, u8)>,
    core_series: &[CoreLine],
    ping_points: &VecDeque<u16>,
    title: String,
    window: ChartWindow,
    now_sec: i64,
    view: WeakEntity<CoreStatusView>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let cpu_line = design::moon(p.blue);
    let cpu_fill = design::moon_alpha(p.blue, 0.14);
    let mem_line = design::moon(p.green);
    let mem_fill = design::moon_alpha(p.green, 0.14);
    let ping_line = design::moon(p.accent);
    let grid = design::moon_alpha(p.text_muted, 0.16);

    let span = window.secs();
    let cpu = to_series(server_points, span, |s| s.0 as f32);
    let mem = to_series(server_points, span, |s| s.1 as f32);
    let cur_cpu = server_points.back().map(|(cpu, _)| *cpu);
    let cur_mem = server_points.back().map(|(_, mem)| *mem);

    // Ping rides its own auto-scaled ms axis (round-trip is not a percentage), mapped into the same
    // 0..100 plot height and rounded up to a tidy 50 ms step so the line does not hug the ceiling.
    let cur_ping = ping_points.back().copied();
    let ping_scale = (ping_points.iter().copied().max().unwrap_or(0) as f32 / 50.0)
        .ceil()
        .max(1.0)
        * 50.0;
    let ping = to_series(ping_points, span, |ms| *ms as f32 / ping_scale * 100.0);

    let core_draws: Vec<CoreDraw> = core_series
        .iter()
        .map(|core| CoreDraw {
            cpu: to_series(&core.points, span, |s| s.0 as f32),
            mem: to_series(&core.points, span, |s| s.1 as f32),
            line: design::moon(core.color),
            faint: design::moon_alpha(core.color, CORE_MEM_ALPHA),
        })
        .collect();

    let plot = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let w = f32::from(bounds.size.width);
            let h = f32::from(bounds.size.height);
            if w < 4.0 || h < 4.0 {
                return;
            }
            let y = |value: f32| bounds.origin.y + px((100.0 - value) / 100.0 * (h - 2.0) + 1.0);
            for line in GRID {
                window.paint_quad(gpui::fill(
                    Bounds::new(
                        gpui::point(bounds.origin.x, y(f32::from(line))),
                        gpui::size(px(w), px(1.0)),
                    ),
                    grid,
                ));
            }
            // Vertical time grid: a fixed number of divisions regardless of the window span.
            for k in 1..GRID_DIVISIONS {
                let vx = bounds.origin.x + px(k as f32 / GRID_DIVISIONS as f32 * w);
                window.paint_quad(gpui::fill(
                    Bounds::new(gpui::point(vx, bounds.origin.y), gpui::size(px(1.0), px(h))),
                    grid,
                ));
            }
            // Cores underneath (memory fainter than CPU), then the server pair on top.
            for core in &core_draws {
                paint_series(
                    window,
                    bounds.origin,
                    w,
                    h,
                    &core.mem,
                    None,
                    core.faint,
                    CORE_STROKE,
                );
                paint_series(
                    window,
                    bounds.origin,
                    w,
                    h,
                    &core.cpu,
                    None,
                    core.line,
                    CORE_STROKE,
                );
            }
            paint_series(
                window,
                bounds.origin,
                w,
                h,
                &mem,
                Some(mem_fill),
                mem_line,
                SERVER_STROKE,
            );
            paint_series(
                window,
                bounds.origin,
                w,
                h,
                &cpu,
                Some(cpu_fill),
                cpu_line,
                SERVER_STROKE,
            );
            // Ping on top, unfilled (its own ms scale), so its trend reads over the % series.
            paint_series(
                window,
                bounds.origin,
                w,
                h,
                &ping,
                None,
                ping_line,
                SERVER_STROKE,
            );
        },
    )
    .size_full();

    // Plot area: canvas inset by the axis gutters, with overlay labels aligned to the same Y map.
    let plot_area = div()
        .relative()
        .w_full()
        .h(px(PLOT_H))
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(AXIS_W))
                .right(px(VAL_W))
                .child(plot),
        )
        .children(GRID.iter().rev().map(|&v| axis_label(v, p, cx)))
        .children(cur_cpu.map(|v| value_label(v, cpu_line, cx)))
        .children(cur_mem.map(|v| value_label(v, mem_line, cx)))
        .children(cur_ping.map(|ms| ping_value_label(ms, ping_scale, ping_line, cx)));

    v_flex()
        .flex_none()
        .w_full()
        .px_2()
        .py_1()
        .gap_1()
        .text_size(design::t_caption(cx))
        .child(legend_row(
            title, window, &view, cpu_line, mem_line, ping_line, p,
        ))
        .children((!core_series.is_empty()).then(|| core_legend_row(core_series, p)))
        .child(plot_area)
        .child(x_axis_row(now_sec, window, p, cx))
        .into_any_element()
}

/// Paint one series (x-fraction points) as an optional bottom-filled area plus a stroked line on a
/// 0..100 % Y.
fn paint_series(
    window: &mut Window,
    origin: Point<Pixels>,
    w: f32,
    h: f32,
    series: &[(f32, f32)],
    fill_col: Option<Hsla>,
    line_col: Hsla,
    width: f32,
) {
    if series.len() < 2 {
        return;
    }
    let x = |frac: f32| origin.x + px(frac * w);
    let y = |value: f32| origin.y + px((100.0 - value) / 100.0 * (h - 2.0) + 1.0);

    if let Some(fill_col) = fill_col {
        let y0 = y(0.0);
        let mut fill = PathBuilder::fill();
        fill.move_to(gpui::point(x(series[0].0), y0));
        for &(frac, value) in series {
            fill.line_to(gpui::point(x(frac), y(value)));
        }
        fill.line_to(gpui::point(x(series[series.len() - 1].0), y0));
        if let Ok(path) = fill.build() {
            window.paint_path(path, fill_col);
        }
    }

    let mut line = PathBuilder::stroke(px(width));
    for (k, &(frac, value)) in series.iter().enumerate() {
        let pt = gpui::point(x(frac), y(value));
        if k == 0 {
            line.move_to(pt);
        } else {
            line.line_to(pt);
        }
    }
    if let Ok(path) = line.build() {
        window.paint_path(path, line_col);
    }
}

/// One left-gutter percent label aligned to its grid line.
fn axis_label(value: u8, p: MoonPalette, cx: &App) -> impl IntoElement {
    let top = (100.0 - f32::from(value)) / 100.0 * PLOT_H;
    div()
        .absolute()
        .left_0()
        .top(px(top - 6.0))
        .w(px(AXIS_W - 4.0))
        .flex()
        .justify_end()
        .text_size(design::t_caption(cx))
        .text_color(rgb(p.text_muted))
        .child(value.to_string())
}

/// One right-gutter current-value label at the series' latest point.
fn value_label(value: u8, color: Hsla, cx: &App) -> impl IntoElement {
    let top = (100.0 - f32::from(value)) / 100.0 * PLOT_H;
    div()
        .absolute()
        .right_0()
        .top(px(top - 6.0))
        .w(px(VAL_W))
        .pl_1()
        .text_size(design::t_caption(cx))
        .text_color(color)
        .child(format!("{value}%"))
}

/// Right-gutter current-ping label, placed on the ping line's own ms scale (`ping_scale` = 100 %).
fn ping_value_label(ms: u16, ping_scale: f32, color: Hsla, cx: &App) -> impl IntoElement {
    let pct = (f32::from(ms) / ping_scale * 100.0).clamp(0.0, 100.0);
    let top = (100.0 - pct) / 100.0 * PLOT_H;
    div()
        .absolute()
        .right_0()
        .top(px(top - 6.0))
        .w(px(VAL_W))
        .pl_1()
        .text_size(design::t_caption(cx))
        .text_color(color)
        .child(format!("{} {}", ms, t!("core_status.ms")))
}

/// Legend row: server title, window buttons, and the two server series color chips.
fn legend_row(
    title: String,
    window: ChartWindow,
    view: &WeakEntity<CoreStatusView>,
    cpu_line: Hsla,
    mem_line: Hsla,
    ping_line: Hsla,
    p: MoonPalette,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_center()
        .gap_2()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(rgb(p.text_soft))
                .child(title),
        )
        .child(window_button(ChartWindow::Min5, window, view, p))
        .child(window_button(ChartWindow::Hour1, window, view, p))
        .child(legend_chip(
            t!("core_status.chart_cpu").to_string(),
            cpu_line,
            p,
        ))
        .child(legend_chip(
            t!("core_status.chart_mem").to_string(),
            mem_line,
            p,
        ))
        .child(legend_chip(
            t!("core_status.chart_ping").to_string(),
            ping_line,
            p,
        ))
}

/// Second legend row: one wrapped chip per core, in the same hue as its line pair.
fn core_legend_row(core_series: &[CoreLine], p: MoonPalette) -> impl IntoElement {
    h_flex().w_full().flex_wrap().gap_x_2().gap_y_1().children(
        core_series
            .iter()
            .map(|core| legend_chip(core.name.clone(), design::moon(core.color), p)),
    )
}

/// One window-selector pill; the active window is accent-tinted.
fn window_button(
    which: ChartWindow,
    current: ChartWindow,
    view: &WeakEntity<CoreStatusView>,
    p: MoonPalette,
) -> impl IntoElement {
    let active = which == current;
    let view = view.clone();
    div()
        .id(SharedString::from(match which {
            ChartWindow::Min5 => "cs-chart-5m",
            ChartWindow::Hour1 => "cs-chart-1h",
        }))
        .px_1p5()
        .rounded(px(4.0))
        .cursor_pointer()
        .when(active, |el| el.bg(design::moon_alpha(p.accent, 0.16)))
        .text_color(rgb(if active { p.accent } else { p.text_soft }))
        .child(t!(which.label_key()).to_string())
        .on_click(move |_, _, app| {
            if let Some(view) = view.upgrade() {
                view.update(app, |this, cx| this.set_chart_window(which, cx));
            }
        })
}

/// One legend entry: a colored dot and its label.
fn legend_chip(label: String, color: Hsla, p: MoonPalette) -> impl IntoElement {
    h_flex()
        .flex_none()
        .items_center()
        .gap_1()
        .child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(color))
        .child(div().text_color(rgb(p.text_muted)).child(label))
}

/// X-axis time labels (HH:MM:SS, UTC) placed under the vertical grid divisions.
fn x_axis_row(now_sec: i64, window: ChartWindow, p: MoonPalette, cx: &App) -> impl IntoElement {
    let span = window.secs() as i64;
    // Reserve the gutters so labels line up with the plot, then place them by fraction inside.
    h_flex().w_full().pl(px(AXIS_W)).pr(px(VAL_W)).child(
        div()
            .relative()
            .w_full()
            .h(px(14.0))
            .text_size(design::t_caption(cx))
            .text_color(rgb(p.text_muted))
            .children((0..=GRID_DIVISIONS).step_by(LABEL_EVERY).map(|k| {
                let frac = k as f32 / GRID_DIVISIONS as f32;
                // Right edge (frac 1) is "now"; the left edge is one window back.
                let secs_ago = ((1.0 - frac) * span as f32).round() as i64;
                // Nudge end labels inward so neither hangs past the plot.
                let shift = if k == 0 {
                    px(0.0)
                } else if k == GRID_DIVISIONS {
                    px(-48.0)
                } else {
                    px(-24.0)
                };
                div()
                    .absolute()
                    .left(relative(frac))
                    .ml(shift)
                    .whitespace_nowrap()
                    .child(hms(now_sec - secs_ago))
            })),
    )
}

/// Format a Unix second as `HH:MM:SS` (UTC), matching the header clock's day-of arithmetic.
fn hms(unix_sec: i64) -> String {
    let day = unix_sec.rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", day / 3600, (day % 3600) / 60, day % 60)
}
