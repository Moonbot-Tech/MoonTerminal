//! Charts of the "Summary" tab: cumulative profit (a vector area + line on
//! canvas — the path is built from the element's real bounds, like the detect
//! sparklines) and daily bars (divs, green/orange by sign).

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};

use super::super::AnalyticsView;
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::{CoreSeries, DayPoint};

const CHART_H: f32 = 170.0;

/// FALLBACK color for a core's series (cycled from the palette) — used when
/// the server has no color in its settings (e.g. the core is already gone from
/// the config). The primary source is `ServerConfig.color` (see core_colors in
/// summary.rs).
pub(super) fn fallback_core_color(p: MoonPalette, i: usize) -> u32 {
    [
        p.blue,
        p.green,
        p.orange,
        p.amber,
        p.red,
        p.yellow,
        p.accent,
        p.text_soft,
    ][i % 8]
}

/// "dd.mm" from unix seconds (axis labels).
fn dm(secs: i64) -> String {
    let s = moon_core::db::fmt_unix(secs);
    if s.len() >= 10 {
        format!("{}.{}", &s[8..10], &s[5..7])
    } else {
        s
    }
}

/// Cumulative profit: an area fill with a stroked line on top.
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
            let y0 = y(vmin.min(0.0).max(vmin)); // area baseline = the bottom (min, but never above 0)
            // The area down to the baseline.
            let mut fill = PathBuilder::fill();
            fill.move_to(gpui::point(x(0), y0));
            for (k, &v) in pts_fill.iter().enumerate() {
                fill.line_to(gpui::point(x(k), y(v)));
            }
            fill.line_to(gpui::point(x(n - 1), y0));
            if let Ok(path) = fill.build() {
                window.paint_path(path, fill_col);
            }
            // Zero line, if the curve ever went negative.
            if vmin < 0.0 {
                window.paint_quad(gpui::fill(
                    Bounds::new(
                        gpui::point(bounds.origin.x, y(0.0)),
                        gpui::size(px(w), px(1.0)),
                    ),
                    moon_alpha(p.text_muted, 0.5),
                ));
            }
            // The line on top of the area.
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

/// Daily profit bars: green upward / orange downward from the zero line, with
/// the value labelled above a green bar / below a red one (while the bars are
/// few). Hovering a column shows the same per-core popup as the left chart.
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
    let vmax = days
        .iter()
        .map(|d| d.profit)
        .fold(0.0f64, f64::max)
        .max(1e-6);
    let vmin = days
        .iter()
        .map(|d| d.profit)
        .fold(0.0f64, f64::min)
        .min(0.0);
    let span = (vmax - vmin).max(1e-6);
    let up_frac = (vmax / span) as f32; // share of the height above the zero line
    // Value labels stay readable only while the bars are few.
    let labels_on = days.len() <= 45;
    // Space reserved for the labels: always on top (above the tallest green
    // bar), on the bottom only when there is a negative value (the label goes
    // BELOW a red bar). Bars scale into the remaining height, so the numbers
    // are never covered by a column.
    let pad_top = if labels_on { 13.0f32 } else { 0.0 };
    let pad_bottom = if labels_on && vmin < 0.0 {
        13.0f32
    } else {
        0.0
    };
    let area_h = (CHART_H - pad_top - pad_bottom).max(10.0);
    let zero_from_bottom = pad_bottom + area_h * (1.0 - up_frac);
    let n = days.len();

    let mut row = h_flex()
        .w_full()
        .h(px(CHART_H))
        .items_end()
        .gap(px(if n > 120 { 0.0 } else { 1.0 }));
    for (bi, d) in days.iter().enumerate() {
        let frac = (d.profit.abs() / span) as f32;
        let bar_h = (frac * area_h).max(if d.trades > 0 { 1.5 } else { 0.0 });
        // Positives grow up from the zero line, negatives grow down.
        let bottom = if d.profit >= 0.0 {
            zero_from_bottom
        } else {
            (zero_from_bottom - bar_h).max(pad_bottom - 13.0)
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
            // Label: above a green bar / below a red one (the space is
            // reserved by pad_top/pad_bottom, so no bar covers the number).
            let label_bottom = if d.profit >= 0.0 {
                bottom + bar_h + 2.0
            } else {
                (bottom - 12.0).max(0.0)
            };
            // The label is wider than its column (±24px on each side) and does
            // not wrap — otherwise "333" got clipped to "33". Neighbouring
            // labels may touch slightly, but every number stays fully readable.
            col = col.child(
                div()
                    .absolute()
                    .left(px(-24.0))
                    .right(px(-24.0))
                    .bottom(px(label_bottom))
                    .text_size(px(8.0))
                    .whitespace_nowrap()
                    .text_color(moon(super::sign_color(p, d.profit)))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .justify_center()
                            .child(moon_core::util::fmt::compact(d.profit, 0)),
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
        .child(v_flex().w_full().gap(px(4.0)).child(row).child(axis_row(
            p,
            dm(first),
            dm(last),
            None,
        )))
        .children(popup)
        .into_any_element()
}

/// Cumulative profit PER CORE: one line per core (on the same bucket grid as
/// the total cumulative chart) + a legend with the totals. `h` is the canvas
/// height. `hover` is the bucket under the mouse: the column is caught by an
/// invisible overlay, and the popup shows each core's profit on that date.
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
    // Cumulative curve per core + the shared Y range.
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
            // Zero line.
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
    // Invisible hover-catcher columns over the canvas (one bucket per column).
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
    // Grand total of all cores goes in the axis label (as on the total area).
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
                // No legend here on purpose: core names and totals are visible
                // in the per-date popup and in the bottom "Profit by core" chart.
                .child(axis_row(p, dm(first), dm(last), Some(total_all as f32))),
        )
        .children(popup)
        .into_any_element()
}

/// Popup of bucket `bi`'s values: the date + each core's profit (descending)
/// + the total. Anchored to the date column inside the chart's relative container.
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
    // Sorted BY PROFIT: profitable cores on top, losing ones at the bottom.
    items.sort_by(|a, b| b.1.total_cmp(&a.1));
    let total: f64 = items.iter().map(|(_, v)| *v).sum();
    // The day's total goes IN THE POPUP HEADER (with dozens of cores the
    // bottom of the list is off screen).
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
                                .text_color(moon(super::sign_color(p, total)))
                                .child(super::fmt_signed(total)),
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
                        .bg(colors
                            .get(ci)
                            .copied()
                            .unwrap_or_else(|| moon(fallback_core_color(p, ci)))),
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
                        .text_color(moon(super::sign_color(p, v)))
                        .child(super::fmt_signed(v)),
                ),
        );
    }
    // Anchored to the date column: in the right third it goes to the left of
    // it, otherwise to the right. deferred — the popup paints ON TOP of
    // everything (otherwise it hid under the bottom cards, drawn later).
    let frac = bi as f32 / days.len().max(1) as f32;
    let mut holder = div()
        .absolute()
        .top(px(6.0))
        .w(design::font_w_px(cx, 190.0));
    if frac <= 0.62 {
        holder = holder.left(relative(frac)).ml(px(12.0));
    } else {
        holder = holder.right(relative(1.0 - frac)).mr(px(12.0));
    }
    deferred(holder.child(card)).into_any_element()
}

/// Period total PER CORE: one bar per core (the SUM of profit over the
/// period), with the core's name and number under it. The canvas is ELASTIC:
/// the bars are painted on canvas and stretch across the full height the
/// bottom-pinned panel gives them.
pub(super) fn core_totals_bars(
    cores: &[CoreSeries],
    colors: &[Hsla],
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    if cores.is_empty() {
        return div().flex_1().into_any_element();
    }
    let vmax = cores
        .iter()
        .map(|c| c.total)
        .fold(0.0f64, f64::max)
        .max(1e-6);
    let vmin = cores
        .iter()
        .map(|c| c.total)
        .fold(0.0f64, f64::min)
        .min(0.0);
    let span = (vmax - vmin).max(1e-6);
    let up_frac = (vmax / span) as f32;
    let gap = f32::from(design::ui_px(cx, 8.0));
    // Bar color follows the sign (profit/loss), as on the daily chart; the
    // core's own color lives in the dot of the label under the bar.
    let bars: Vec<f32> = cores.iter().map(|c| c.total as f32).collect();
    let up_col = moon(p.green);
    let down_col = moon(p.orange);
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
            // Zero line when there are losing cores.
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
            for (k, v) in bars.iter().enumerate() {
                let x = bounds.origin.x + px(k as f32 * (col_w + gap));
                let bar_h = (v.abs() / span32 * h).max(if v.abs() > 1e-9 { 1.5 } else { 0.0 });
                let top = if *v >= 0.0 {
                    zero_y - px(bar_h)
                } else {
                    zero_y
                };
                window.paint_quad(gpui::fill(
                    Bounds::new(gpui::point(x, top), gpui::size(px(col_w), px(bar_h))),
                    if *v >= 0.0 { up_col } else { down_col },
                ));
            }
        },
    )
    .w_full()
    .flex_1()
    .min_h(px(60.0));

    // Labels under the bars use the same column grid (flex_1 + the same gap).
    let mut labels = h_flex().w_full().flex_none().gap(px(gap));
    for (ci, c) in cores.iter().enumerate() {
        let v = c.total;
        let dot_col = colors
            .get(ci)
            .copied()
            .unwrap_or_else(|| moon(fallback_core_color(p, ci)));
        labels = labels.child(
            v_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .child(
                    // Keep the core-color dot beside the name because the bar
                    // color represents profit sign rather than core identity.
                    h_flex()
                        .max_w_full()
                        .items_center()
                        .gap(design::ui_px(cx, 4.0))
                        .child(
                            div()
                                .flex_none()
                                .w(design::ui_px(cx, 6.0))
                                .h(design::ui_px(cx, 6.0))
                                .rounded_full()
                                .bg(dot_col),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(crate::design::t_caption(cx))
                                .text_color(moon(p.text_soft))
                                .child(c.name.clone()),
                        ),
                )
                .child(
                    // The total is larger than the name (the chart's headline number).
                    div()
                        .whitespace_nowrap()
                        .text_size(crate::design::t_body(cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(moon(super::sign_color(p, v)))
                        .child(super::fmt_signed(v)),
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

/// X-axis labels: the first/last date (+ the total on the right for the
/// cumulative chart).
fn axis_row(p: MoonPalette, left: String, right: String, total: Option<f32>) -> AnyElement {
    let mut row = h_flex()
        .w_full()
        .justify_between()
        .child(muted_caption(p, left));
    if let Some(t) = total {
        row = row.child(
            div()
                .text_color(moon(super::sign_color(p, t as f64)))
                .child(super::fmt_signed(t as f64)),
        );
    }
    row.child(muted_caption(p, right)).into_any_element()
}

fn muted_caption(p: MoonPalette, text: String) -> Div {
    div().text_color(moon(p.text_muted)).child(text)
}
