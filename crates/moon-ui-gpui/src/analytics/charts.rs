//! Графики вкладки «Сводка»: накопительная прибыль (векторная область+линия,
//! canvas — путь строится по реальным границам элемента, как спарклайны
//! детектов) и дневные бары (div'ы, зелёный/оранжевый по знаку).

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};

use super::AnalyticsView;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::DayPoint;

const CHART_H: f32 = 170.0;

/// «дд.мм» из unix-секунд (подписи осей).
fn dm(secs: i64) -> String {
    let s = moon_core::db::fmt_unix(secs);
    if s.len() >= 10 {
        format!("{}.{}", &s[8..10], &s[5..7])
    } else {
        s
    }
}

/// Накопительная прибыль: заливка области + линия-штрих сверху.
pub(super) fn cumulative_area(days: &[DayPoint], p: MoonPalette) -> AnyElement {
    if days.is_empty() {
        return div().h(px(CHART_H)).into_any_element();
    }
    let mut cum = 0.0f32;
    let pts: Vec<f32> = days
        .iter()
        .map(|d| {
            cum += d.profit as f32;
            cum
        })
        .collect();
    let vmax = pts.iter().copied().fold(0.0f32, f32::max).max(1e-6);
    let vmin = pts.iter().copied().fold(0.0f32, f32::min).min(0.0);
    let span = (vmax - vmin).max(1e-6);
    let total = *pts.last().unwrap_or(&0.0);
    let line_col = moon(if total >= 0.0 { p.green } else { p.orange });
    let fill_col = moon_alpha(if total >= 0.0 { p.green } else { p.orange }, 0.14);
    let first = days.first().map(|d| d.start).unwrap_or(0);
    let last = days.last().map(|d| d.start).unwrap_or(0);

    let pts_fill = pts.clone();
    let canvas_el = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let w = f32::from(bounds.size.width);
            let h = f32::from(bounds.size.height);
            if w < 4.0 || h < 4.0 || pts_fill.len() < 2 {
                return;
            }
            let n = pts_fill.len();
            let x = |k: usize| bounds.origin.x + px(k as f32 / (n - 1) as f32 * w);
            let y = |v: f32| bounds.origin.y + px((vmax - v) / span * (h - 2.0) + 1.0);
            let y0 = y(vmin.min(0.0).max(vmin)); // базовая линия области = низ (min, но не выше 0)
            // Область до низа.
            let mut fill = PathBuilder::fill();
            fill.move_to(gpui::point(x(0), y0));
            for (k, &v) in pts_fill.iter().enumerate() {
                fill.line_to(gpui::point(x(k), y(v)));
            }
            fill.line_to(gpui::point(x(n - 1), y0));
            if let Ok(path) = fill.build() {
                window.paint_path(path, fill_col);
            }
            // Нулевая линия, если кривая уходила в минус.
            if vmin < 0.0 {
                window.paint_quad(gpui::fill(
                    Bounds::new(
                        gpui::point(bounds.origin.x, y(0.0)),
                        gpui::size(px(w), px(1.0)),
                    ),
                    moon_alpha(p.text_muted, 0.5),
                ));
            }
            // Линия поверх области.
            let mut pb = PathBuilder::stroke(px(2.0));
            for (k, &v) in pts_fill.iter().enumerate() {
                let pt = gpui::point(x(k), y(v));
                if k == 0 {
                    pb.move_to(pt);
                } else {
                    pb.line_to(pt);
                }
            }
            if let Ok(path) = pb.build() {
                window.paint_path(path, line_col);
            }
        },
    )
    .w_full()
    .h(px(CHART_H));

    v_flex()
        .w_full()
        .gap(px(4.0))
        .child(canvas_el)
        .child(axis_row(p, dm(first), dm(last), Some(total)))
        .into_any_element()
}

/// Дневные бары профита: зелёный вверх / оранжевый вниз от нулевой линии.
pub(super) fn daily_bars(
    days: &[DayPoint],
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    if days.is_empty() {
        return div().h(px(CHART_H)).into_any_element();
    }
    let vmax = days.iter().map(|d| d.profit).fold(0.0f64, f64::max).max(1e-6);
    let vmin = days.iter().map(|d| d.profit).fold(0.0f64, f64::min).min(0.0);
    let span = (vmax - vmin).max(1e-6);
    let up_frac = (vmax / span) as f32; // доля высоты над нулевой линией

    let mut row = h_flex()
        .w_full()
        .h(px(CHART_H))
        .items_end()
        .gap(px(if days.len() > 120 { 0.0 } else { 1.0 }));
    for d in days {
        let frac = (d.profit.abs() / span) as f32;
        let bar_h = (frac * CHART_H).max(if d.trades > 0 { 1.5 } else { 0.0 });
        // Смещение бара: положительные растут от нулевой линии вверх,
        // отрицательные — вниз (низ бара = нулевая линия минус высота).
        let zero_from_bottom = CHART_H * (1.0 - up_frac);
        let mb = if d.profit >= 0.0 {
            zero_from_bottom
        } else {
            (zero_from_bottom - bar_h).max(0.0)
        };
        row = row.child(
            div()
                .flex_1()
                .h(px(bar_h))
                .mb(px(mb))
                .rounded(px(1.0))
                .bg(moon(if d.profit >= 0.0 { p.green } else { p.orange })),
        );
    }
    let first = days.first().map(|d| d.start).unwrap_or(0);
    let last = days.last().map(|d| d.start).unwrap_or(0);
    let _ = cx;
    v_flex()
        .w_full()
        .gap(px(4.0))
        .child(row)
        .child(axis_row(p, dm(first), dm(last), None))
        .into_any_element()
}

/// Подписи оси X: первая/последняя дата (+ итог справа у накопительной).
fn axis_row(p: MoonPalette, left: String, right: String, total: Option<f32>) -> AnyElement {
    let mut row = h_flex()
        .w_full()
        .justify_between()
        .child(muted_caption(p, left));
    if let Some(t) = total {
        row = row.child(
            div()
                .text_color(moon(super::summary::sign_color(p, t as f64)))
                .child(super::summary::fmt_signed(t as f64)),
        );
    }
    row.child(muted_caption(p, right)).into_any_element()
}

fn muted_caption(p: MoonPalette, text: String) -> Div {
    div().text_color(moon(p.text_muted)).child(text)
}
