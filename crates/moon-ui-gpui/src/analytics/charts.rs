//! Графики вкладки «Сводка»: накопительная прибыль (векторная область+линия,
//! canvas — путь строится по реальным границам элемента, как спарклайны
//! детектов) и дневные бары (div'ы, зелёный/оранжевый по знаку).

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};

use super::AnalyticsView;
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::{CoreSeries, DayPoint};

const CHART_H: f32 = 170.0;


/// ФОЛБЭК-цвет серии ядра (циклом из палитры) — когда у сервера нет цвета в
/// настройках (например, ядро уже удалено из конфига). Основной источник —
/// `ServerConfig.color` (см. core_colors в summary.rs).
pub(super) fn fallback_core_color(p: MoonPalette, i: usize) -> u32 {
    [p.blue, p.green, p.orange, p.amber, p.red, p.yellow, p.accent, p.text_soft]
        [i % 8]
}

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

/// Дневные бары профита: зелёный вверх / оранжевый вниз от нулевой линии,
/// подпись значения над зелёным / под красным (пока баров немного), ховер
/// колонки — тот же попап по ядрам, что у левого чарта.
pub(super) fn daily_bars(
    days: &[DayPoint],
    cores: &[CoreSeries],
    colors: &[Hsla],
    hover: Option<usize>,
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
    let zero_from_bottom = CHART_H * (1.0 - up_frac);
    // Подписи значений читаемы только пока баров немного.
    let labels_on = days.len() <= 45;
    let n = days.len();

    let mut row = h_flex()
        .w_full()
        .h(px(CHART_H))
        .items_end()
        .gap(px(if n > 120 { 0.0 } else { 1.0 }));
    for (bi, d) in days.iter().enumerate() {
        let frac = (d.profit.abs() / span) as f32;
        let bar_h = (frac * CHART_H).max(if d.trades > 0 { 1.5 } else { 0.0 });
        // Положительные растут от нулевой линии вверх, отрицательные — вниз.
        let bottom = if d.profit >= 0.0 {
            zero_from_bottom
        } else {
            (zero_from_bottom - bar_h).max(0.0)
        };
        let mut col = div()
            .id(SharedString::from(format!("an-db-{bi}")))
            .flex_1()
            .relative()
            .h_full()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    if this.hover_daily_bucket != Some(bi) {
                        this.hover_daily_bucket = Some(bi);
                        cx.notify();
                    }
                } else if this.hover_daily_bucket == Some(bi) {
                    this.hover_daily_bucket = None;
                    cx.notify();
                }
            }))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(bottom))
                    .h(px(bar_h))
                    .rounded(px(1.0))
                    .bg(moon(if d.profit >= 0.0 { p.green } else { p.orange })),
            );
        if hover == Some(bi) {
            col = col.bg(moon_alpha(p.text_muted, 0.07));
        }
        if labels_on && d.trades > 0 {
            // Подпись: над зелёным баром / под красным.
            let label_bottom = if d.profit >= 0.0 {
                (bottom + bar_h + 2.0).min(CHART_H - 11.0)
            } else {
                (bottom - 13.0).max(0.0)
            };
            // Подпись шире колонки (±24px по бокам) и без переносов — иначе
            // «333» обрезалось в «33»; соседние подписи могут чуть касаться,
            // но числа читаемы целиком.
            col = col.child(
                div()
                    .absolute()
                    .left(px(-24.0))
                    .right(px(-24.0))
                    .bottom(px(label_bottom))
                    .text_size(px(8.0))
                    .whitespace_nowrap()
                    .text_color(moon(super::summary::sign_color(p, d.profit)))
                    .child(
                        div().w_full().flex().justify_center().child(
                            moon_core::util::fmt::compact(d.profit, 0),
                        ),
                    ),
            );
        }
        row = row.child(col);
    }
    let popup = hover
        .filter(|bi| *bi < n && !cores.is_empty())
        .map(|bi| core_bucket_popup(days, cores, colors, bi, p, cx));
    let first = days.first().map(|d| d.start).unwrap_or(0);
    let last = days.last().map(|d| d.start).unwrap_or(0);
    div()
        .relative()
        .w_full()
        .child(
            v_flex()
                .w_full()
                .gap(px(4.0))
                .child(row)
                .child(axis_row(p, dm(first), dm(last), None)),
        )
        .children(popup)
        .into_any_element()
}

/// Накопительная прибыль ПО ЯДРАМ: линия на ядро (той же сеткой вёдер, что
/// суммарная накопительная) + легенда с итогами. `h` — высота полотна.
/// `hover` — ведро под мышью: колонка ловится невидимым оверлеем, попап
/// показывает профит каждого ядра в эту дату.
pub(super) fn core_lines(
    days: &[DayPoint],
    cores: &[CoreSeries],
    colors: &[Hsla],
    h: f32,
    hover: Option<usize>,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    if days.is_empty() || cores.is_empty() {
        return div().h(px(h)).into_any_element();
    }
    // Кумулятив по каждому ядру + общий диапазон Y.
    let curves: Vec<Vec<f32>> = cores
        .iter()
        .map(|c| {
            let mut cum = 0.0f32;
            c.per_bucket
                .iter()
                .map(|v| {
                    cum += *v as f32;
                    cum
                })
                .collect()
        })
        .collect();
    let mut vmax = 1e-6f32;
    let mut vmin = 0.0f32;
    for c in &curves {
        for v in c {
            vmax = vmax.max(*v);
            vmin = vmin.min(*v);
        }
    }
    let span = (vmax - vmin).max(1e-6);

    let curves_paint = curves;
    let colors_paint: Vec<Hsla> = colors.to_vec();
    let muted = moon_alpha(p.text_muted, 0.5);
    let canvas_el = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let w = f32::from(bounds.size.width);
            let ch = f32::from(bounds.size.height);
            if w < 4.0 || ch < 4.0 {
                return;
            }
            let y = |v: f32| bounds.origin.y + px((vmax - v) / span * (ch - 2.0) + 1.0);
            // Нулевая линия.
            if vmin < 0.0 {
                window.paint_quad(gpui::fill(
                    Bounds::new(
                        gpui::point(bounds.origin.x, y(0.0)),
                        gpui::size(px(w), px(1.0)),
                    ),
                    muted,
                ));
            }
            for (ci, pts) in curves_paint.iter().enumerate() {
                if pts.len() < 2 {
                    continue;
                }
                let n = pts.len();
                let x = |k: usize| bounds.origin.x + px(k as f32 / (n - 1) as f32 * w);
                let mut pb = PathBuilder::stroke(px(1.6));
                for (k, &v) in pts.iter().enumerate() {
                    let pt = gpui::point(x(k), y(v));
                    if k == 0 {
                        pb.move_to(pt);
                    } else {
                        pb.line_to(pt);
                    }
                }
                if let Ok(path) = pb.build() {
                    window.paint_path(path, colors_paint[ci]);
                }
            }
        },
    )
    .w_full()
    .h(px(h));

    let first = days.first().map(|d| d.start).unwrap_or(0);
    let last = days.last().map(|d| d.start).unwrap_or(0);
    // Невидимые колонки-ловушки ховера поверх полотна (по ведру на колонку).
    let n = days.len();
    let mut hover_row = h_flex().absolute().inset_0().gap_0();
    for bi in 0..n {
        let mut col = div()
            .id(SharedString::from(format!("an-cl-{bi}")))
            .flex_1()
            .h_full()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    if this.hover_core_bucket != Some(bi) {
                        this.hover_core_bucket = Some(bi);
                        cx.notify();
                    }
                } else if this.hover_core_bucket == Some(bi) {
                    this.hover_core_bucket = None;
                    cx.notify();
                }
            }));
        if hover == Some(bi) {
            col = col.bg(moon_alpha(p.text_muted, 0.07));
        }
        hover_row = hover_row.child(col);
    }
    let popup = hover
        .filter(|bi| *bi < n)
        .map(|bi| core_bucket_popup(days, cores, colors, bi, p, cx));
    // Общий итог всех ядер — в подписи оси (как у суммарной области).
    let total_all: f64 = cores.iter().map(|c| c.total).sum();
    div()
        .relative()
        .w_full()
        .child(
            v_flex()
                .w_full()
                .gap(px(4.0))
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(h))
                        .child(canvas_el)
                        .child(hover_row),
                )
                // Легенды тут нет намеренно: имена/итоги ядер видны в
                // попапе по датам и в нижнем чарте «Прибыль по ядрам».
                .child(axis_row(p, dm(first), dm(last), Some(total_all as f32))),
        )
        .children(popup)
        .into_any_element()
}

/// Попап значений ведра `bi`: дата + профит каждого ядра (по модулю, убыв.)
/// + итог. Якорится к колонке даты внутри relative-контейнера чарта.
fn core_bucket_popup(
    days: &[DayPoint],
    cores: &[CoreSeries],
    colors: &[Hsla],
    bi: usize,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    let mut items: Vec<(usize, f64)> = cores
        .iter()
        .enumerate()
        .map(|(ci, c)| (ci, c.per_bucket[bi]))
        .filter(|(_, v)| v.abs() > 1e-9)
        .collect();
    // Сортировка ПО ПРОФИТУ: прибыльные сверху, убыточные внизу.
    items.sort_by(|a, b| b.1.total_cmp(&a.1));
    let total: f64 = items.iter().map(|(_, v)| *v).sum();
    // Итог дня — В ШАПКЕ попапа (при десятках ядер низ списка не виден).
    let mut card = v_flex()
        .gap(px(2.0))
        .px(design::ui_px(cx, 8.0))
        .py(design::ui_px(cx, 6.0))
        .rounded(design::ui_px(cx, 6.0))
        .bg(moon(p.panel_high))
        .border_1()
        .border_color(moon(p.border))
        .shadow_md()
        .text_size(crate::design::t_caption(cx))
        .child(
            h_flex()
                .justify_between()
                .gap(design::ui_px(cx, 10.0))
                .pb(px(1.0))
                .border_b_1()
                .border_color(moon_alpha(p.border, 0.6))
                .child(div().text_color(moon(p.text)).child(dm(days[bi].start)))
                .child(
                    h_flex()
                        .gap(design::ui_px(cx, 4.0))
                        .child(div().text_color(moon(p.text_muted)).child("Σ"))
                        .child(
                            div()
                                .text_color(moon(super::summary::sign_color(p, total)))
                                .child(super::summary::fmt_signed(total)),
                        ),
                ),
        );
    for (ci, v) in items.into_iter().take(16) {
        card = card.child(
            h_flex()
                .gap(design::ui_px(cx, 5.0))
                .items_center()
                .child(
                    div()
                        .flex_none()
                        .w(design::ui_px(cx, 6.0))
                        .h(design::ui_px(cx, 6.0))
                        .rounded_full()
                        .bg(colors.get(ci).copied().unwrap_or_else(|| {
                            moon(fallback_core_color(p, ci))
                        })),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(moon(p.text_soft))
                        .child(cores[ci].name.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(moon(super::summary::sign_color(p, v)))
                        .child(super::summary::fmt_signed(v)),
                ),
        );
    }
    // Якорь к колонке даты: в правой трети — слева от неё, иначе справа.
    // deferred — попап рисуется ПОВЕРХ всего (иначе прятался под нижними
    // карточками, отрисованными позже).
    let frac = bi as f32 / days.len().max(1) as f32;
    let mut holder = div().absolute().top(px(6.0)).w(design::font_w_px(cx, 190.0));
    if frac <= 0.62 {
        holder = holder.left(relative(frac)).ml(px(12.0));
    } else {
        holder = holder.right(relative(1.0 - frac)).mr(px(12.0));
    }
    deferred(holder.child(card)).into_any_element()
}

/// Итог периода ПО ЯДРАМ: один столбик на ядро (СУММА профита за период),
/// под столбиком — имя ядра и число. Полотно РЕЗИНОВОЕ: бары рисуются
/// canvas'ом и тянутся на всю высоту, которую даёт прибитая к низу плашка.
pub(super) fn core_totals_bars(
    cores: &[CoreSeries],
    colors: &[Hsla],
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    if cores.is_empty() {
        return div().flex_1().into_any_element();
    }
    let vmax = cores.iter().map(|c| c.total).fold(0.0f64, f64::max).max(1e-6);
    let vmin = cores.iter().map(|c| c.total).fold(0.0f64, f64::min).min(0.0);
    let span = (vmax - vmin).max(1e-6);
    let up_frac = (vmax / span) as f32;
    let gap = f32::from(design::ui_px(cx, 8.0));
    let bars: Vec<(f32, Hsla)> = cores
        .iter()
        .enumerate()
        .map(|(ci, c)| {
            let col = colors
                .get(ci)
                .copied()
                .unwrap_or_else(|| moon(fallback_core_color(p, ci)));
            (c.total as f32, col)
        })
        .collect();
    let muted = moon_alpha(p.text_muted, 0.5);
    let canvas_el = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let w = f32::from(bounds.size.width);
            let h = f32::from(bounds.size.height);
            let n = bars.len();
            if w < 4.0 || h < 4.0 || n == 0 {
                return;
            }
            let col_w = ((w - gap * (n as f32 - 1.0)) / n as f32).max(1.0);
            let zero_y = bounds.origin.y + px(up_frac * (h - 1.0));
            // Нулевая линия при наличии убыточных.
            if vmin < 0.0 {
                window.paint_quad(gpui::fill(
                    Bounds::new(
                        gpui::point(bounds.origin.x, zero_y),
                        gpui::size(px(w), px(1.0)),
                    ),
                    muted,
                ));
            }
            let span32 = span as f32;
            for (k, (v, col)) in bars.iter().enumerate() {
                let x = bounds.origin.x + px(k as f32 * (col_w + gap));
                let bar_h = (v.abs() / span32 * h).max(if v.abs() > 1e-9 { 1.5 } else { 0.0 });
                let top = if *v >= 0.0 { zero_y - px(bar_h) } else { zero_y };
                window.paint_quad(gpui::fill(
                    Bounds::new(gpui::point(x, top), gpui::size(px(col_w), px(bar_h))),
                    *col,
                ));
            }
        },
    )
    .w_full()
    .flex_1()
    .min_h(px(60.0));

    // Подписи под барами — той же сеткой колонок (flex_1 + тот же gap).
    let mut labels = h_flex().w_full().flex_none().gap(px(gap));
    for c in cores {
        let v = c.total;
        labels = labels.child(
            v_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .child(
                    div()
                        .max_w_full()
                        .truncate()
                        .text_size(crate::design::t_caption(cx))
                        .text_color(moon(p.text_soft))
                        .child(c.name.clone()),
                )
                .child(
                    div()
                        .whitespace_nowrap()
                        .text_size(crate::design::t_caption(cx))
                        .text_color(moon(super::summary::sign_color(p, v)))
                        .child(super::summary::fmt_signed(v)),
                ),
        );
    }
    v_flex()
        .size_full()
        .gap(px(3.0))
        .child(canvas_el)
        .child(labels)
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
