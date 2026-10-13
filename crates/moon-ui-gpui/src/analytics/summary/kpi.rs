//! The Summary's KPI band: four groups (Result, Quality, Risk, Activity) drawn from a
//! [`KpiBand`] as a free function over `&App`, so the layout probe renders it without a view.
//!
//! Each group is its own bordered card; Quality, Risk and Activity hold two metric tiles each.
//! Values never truncate: every value cell has a minimum width equal to its measured text.
//! Labels wrap, group captions may shorten, and each comparison part stays on one line.

use gpui::{
    AnyElement, App, Bounds, Div, FontWeight, InteractiveElement, IntoElement, ParentElement,
    PathBuilder, Pixels, StatefulInteractiveElement, Styled, canvas, div, px,
};
use moon_core::util::fmt;
use moon_ui::{MoonPalette, MoonProgress, h_flex, v_flex};
use rust_i18n::t;

use super::model::{DeltaLine, KpiBand};
use super::{fmt_signed, fmt_signed_plain, layout, sign_color};
use crate::design::{self, moon, moon_alpha};

/// How many KPI groups share one row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::analytics) enum KpiCols {
    /// All four groups side by side.
    Four,
    /// Two rows of two.
    Two,
    /// One group per row.
    One,
}

/// Band width at which the four groups share one row, at the design reference size.
const FOUR_COLS_MIN_W: f32 = 1100.0;
/// Band width at which the groups pair up two per row, at the design reference size.
const TWO_COLS_MIN_W: f32 = 700.0;
/// Hero value size relative to the title size, as in the mockup (30px against 18-19px).
const HERO_SCALE: f32 = 1.6;
/// Sparkline height at the design reference size.
const SPARK_H: f32 = 40.0;
/// Narrowest sparkline at the design reference size.
const SPARK_MIN_W: f32 = 100.0;

/// Pick the column layout for a band `width` wide.
///
/// Args:
///     width: Width available to the band, in rendered pixels.
///     scale: Rendered pixels per design-reference pixel (`design::ui_value(cx, 1.0)`).
///
/// Returns:
///     Four at 1100 and wider, Two from 700, One below.
pub(in crate::analytics) fn kpi_cols(width: f32, scale: f32) -> KpiCols {
    if width >= FOUR_COLS_MIN_W * scale {
        KpiCols::Four
    } else if width >= TWO_COLS_MIN_W * scale {
        KpiCols::Two
    } else {
        KpiCols::One
    }
}

/// Mono text at the hero size, for the Result group's figure.
pub(in crate::analytics) fn hero_text_width(cx: &App, text: &str) -> f32 {
    design::ui_text_width_zoomed(
        cx,
        text,
        (design::BODY_TEXT + 3.0) * HERO_SCALE,
        600.0,
        true,
    )
}

/// Whether the drawdown rounds to nothing at the precision it is printed with.
fn dd_is_zero(max_dd: f64) -> bool {
    fmt::round_to(max_dd, 2) == Some(0.0)
}

/// The max-drawdown figure as drawn: a leading minus on the stored magnitude, with the unit; a
/// drawdown that rounds to zero is a plain "0", never "−0".
pub(in crate::analytics) fn dd_text(max_dd: f64) -> String {
    if dd_is_zero(max_dd) {
        return "0".to_string();
    }
    format!(
        "−{}{}",
        fmt::compact(max_dd, 2),
        crate::analytics::pnl_suffix()
    )
}

/// The hero figure: unitless whenever the caption beside it names the unit, so percent mode
/// never prints "%" twice.
pub(in crate::analytics) fn hero_value(profit: f64, unit: &str) -> String {
    if unit.is_empty() {
        fmt_signed(profit)
    } else {
        fmt_signed_plain(profit)
    }
}

/// A ratio (profit factor, recovery factor) at a fixed two decimals, so it reads in step with
/// the two-decimal figures beside it ("2.90", never "2.9"); an em dash when it has no finite
/// value.
pub(in crate::analytics) fn ratio_text(v: f64) -> String {
    fmt::round_to(v, 2).map_or_else(|| "—".to_string(), |r| format!("{r:.2}"))
}

/// The recovery-factor figure, or an em dash when drawdown is zero.
pub(in crate::analytics) fn recovery_text(recovery: Option<f64>) -> String {
    recovery.map_or_else(|| "—".to_string(), ratio_text)
}

/// The trade count with grouped thousands.
pub(in crate::analytics) fn trades_text(n: i64) -> String {
    fmt::group_thousands(&n.to_string())
}

/// The neutral hold-time caption beside the average trade.
pub(in crate::analytics) fn hold_text(minutes: f64) -> String {
    t!(
        "analytics.sum.hold",
        dur = format!("{} {}", fmt::compact(minutes, 0), t!("analytics.minutes"))
    )
    .to_string()
}

/// One named metric of a group: its debug name, label, value text and value colour.
pub(in crate::analytics) struct Metric {
    /// Debug-selector stem; boxes are `sum-kpi-k{name}` and `sum-kpi-v{name}`.
    pub(in crate::analytics) name: &'static str,
    /// Label on top of the tile; it may wrap.
    pub(in crate::analytics) label: String,
    /// Mono value under the label, never shortened.
    pub(in crate::analytics) value: String,
    /// Value colour token.
    pub(in crate::analytics) color: u32,
}

/// The six metric rows of the Quality, Risk and Activity groups, in drawing order.
pub(in crate::analytics) fn metrics(m: &KpiBand, p: MoonPalette) -> [Metric; 6] {
    [
        Metric {
            name: "wr",
            label: t!("analytics.kpi.winrate").to_string(),
            value: fmt::pct(m.quality.winrate, 1).map_or_else(|| "—".to_string(), |v| v.0),
            color: p.text,
        },
        Metric {
            name: "pf",
            label: t!("analytics.kpi.pf").to_string(),
            value: ratio_text(m.quality.pf),
            color: p.text,
        },
        Metric {
            name: "dd",
            label: t!("analytics.kpi.maxdd").to_string(),
            value: dd_text(m.risk.max_dd),
            color: if dd_is_zero(m.risk.max_dd) {
                p.text_soft
            } else {
                p.orange
            },
        },
        Metric {
            name: "rf",
            label: t!("analytics.sum.kpi.recovery").to_string(),
            value: recovery_text(m.risk.recovery),
            color: p.text,
        },
        Metric {
            name: "tr",
            label: t!("analytics.kpi.trades").to_string(),
            value: trades_text(m.activity.trades),
            color: p.text,
        },
        Metric {
            name: "avg",
            label: t!("analytics.kpi.avg").to_string(),
            value: fmt_signed(m.activity.avg),
            color: sign_color(p, m.activity.avg),
        },
    ]
}

/// Render the KPI band.
///
/// Args:
///     m: Band view-model.
///     cols: Column layout from [`kpi_cols`].
///     p: Active palette.
///     cx: App context for scaled sizes and text measurement.
///
/// Returns:
///     Four separately bordered group cards with a gap between them; it schedules nothing.
pub(in crate::analytics) fn kpi_band_el(
    m: &KpiBand,
    cols: KpiCols,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let [wr, pf, dd, rf, tr, avg] = metrics(m, p);
    let pair = |a: Div, b: Div| {
        h_flex()
            .w_full()
            .flex_1()
            .flex_wrap()
            .items_stretch()
            .gap(design::ui_px(cx, 8.0))
            .child(a)
            .child(b)
    };
    let groups = [
        group(0, t!("analytics.sum.kpi.result").to_string(), p, cx).child(result_body(m, p, cx)),
        group(1, t!("analytics.sum.kpi.quality").to_string(), p, cx).child(pair(
            tile(
                &wr,
                meter_el(m, p, cx),
                delta_el("wr", m.quality.wr_delta.as_ref(), None, p, cx),
                p,
                cx,
            ),
            tile(
                &pf,
                None,
                delta_el("pf", m.quality.pf_delta.as_ref(), None, p, cx),
                p,
                cx,
            ),
        )),
        group(2, t!("analytics.sum.kpi.risk").to_string(), p, cx).child(pair(
            tile(
                &dd,
                None,
                delta_el(
                    "dd",
                    m.risk.dd_line.as_ref(),
                    m.risk.dd_of_profit.clone(),
                    p,
                    cx,
                ),
                p,
                cx,
            ),
            tile(
                &rf,
                None,
                Some((
                    muted_line(
                        "rf",
                        t!("analytics.sum.kpi.recovery_hint").to_string(),
                        p,
                        cx,
                    ),
                    0.0,
                )),
                p,
                cx,
            ),
        )),
        group(3, t!("analytics.sum.kpi.activity").to_string(), p, cx).child(pair(
            tile(
                &tr,
                None,
                delta_el("tr", m.activity.trades_delta.as_ref(), None, p, cx),
                p,
                cx,
            ),
            tile(
                &avg,
                None,
                delta_el(
                    "avg",
                    m.activity.avg_delta.as_ref(),
                    Some(hold_text(m.activity.hold_min)),
                    p,
                    cx,
                ),
                p,
                cx,
            ),
        )),
    ];
    let gap = design::ui_px(cx, 12.0);
    let row = || h_flex().w_full().items_stretch().gap(gap);
    let band = v_flex()
        .debug_selector(|| layout::KPI_BAND.into())
        .w_full()
        .gap(gap);
    let [g0, g1, g2, g3] = groups;
    match cols {
        KpiCols::Four => band
            .child(
                row()
                    .child(g0.flex_basis(px(0.0)).flex_grow(1.2))
                    .child(g1.flex_1())
                    .child(g2.flex_1())
                    .child(g3.flex_1()),
            )
            .into_any_element(),
        KpiCols::Two => band
            .child(row().child(g0.flex_1()).child(g1.flex_1()))
            .child(row().child(g2.flex_1()).child(g3.flex_1()))
            .into_any_element(),
        KpiCols::One => band
            .child(g0)
            .child(g1)
            .child(g2)
            .child(g3)
            .into_any_element(),
    }
}

/// One group card: its own border and radius, padding, and an uppercase caption header.
fn group(i: usize, title: String, p: MoonPalette, cx: &App) -> Div {
    v_flex()
        .debug_selector(move || layout::kpi_group(i))
        .min_w_0()
        .bg(moon(p.panel))
        .border_1()
        .border_color(moon(p.border))
        .rounded(design::ui_px(cx, 6.0))
        .px(design::ui_px(cx, 12.0))
        .pt(design::ui_px(cx, 10.0))
        .pb(design::ui_px(cx, 12.0))
        .gap(design::ui_px(cx, 8.0))
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_family(design::ui_font())
                .text_size(design::t_caption(cx))
                .font_weight(FontWeight::MEDIUM)
                .text_color(moon(p.text_muted))
                .child(title.to_uppercase()),
        )
}

/// The Result body: hero figure with unit and sparkline, then the comparison line.
fn result_body(m: &KpiBand, p: MoonPalette, cx: &App) -> Div {
    let r = &m.result;
    let value = hero_value(r.profit, &r.unit);
    let value_w = hero_text_width(cx, &value);
    let unit_w = design::mono_caption_text_width(cx, &r.unit, 400.0);
    let hero_size = px(f32::from(design::t_title(cx)) * HERO_SCALE);
    let hero = h_flex()
        .w_full()
        .flex_wrap()
        .items_end()
        .gap(design::ui_px(cx, 14.0))
        .child(
            h_flex()
                .flex_none()
                .items_end()
                .gap(design::ui_px(cx, 4.0))
                .child(
                    div()
                        .debug_selector(|| layout::kpi_value("hero"))
                        .flex_none()
                        .min_w(px(value_w))
                        .font_family(design::mono())
                        .text_size(hero_size)
                        .line_height(hero_size)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(moon(sign_color(p, r.profit)))
                        .child(value),
                )
                .child(
                    div()
                        .debug_selector(|| layout::kpi_value("unit"))
                        .flex_none()
                        .min_w(px(unit_w))
                        .font_family(design::mono())
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(r.unit.clone()),
                ),
        )
        .child(sparkline(r.spark.clone(), r.profit, p, cx));
    v_flex()
        .gap(design::ui_px(cx, 8.0))
        .child(hero)
        .children(r.was.as_ref().map(|line| {
            h_flex()
                .debug_selector(|| layout::kpi_delta("hero"))
                .flex_wrap()
                .gap_x(design::ui_px(cx, 6.0))
                .font_family(design::mono())
                .text_size(design::t_caption(cx))
                .children(
                    line.prev_text
                        .clone()
                        .map(|prev| div().flex_none().text_color(moon(p.text_soft)).child(prev)),
                )
                .child(div().flex_none().text_color(moon(p.text_muted)).child("·"))
                .child(
                    div()
                        .flex_none()
                        .text_color(moon(line.tone.color(p)))
                        .child(line.text.clone()),
                )
        }))
}

/// Cumulative-profit sparkline: a soft area, a 1.5px line and an endpoint dot.
///
/// Coloured by the period profit's sign, exactly like the hero figure beside it.
fn sparkline(pts: std::rc::Rc<[f32]>, profit: f64, p: MoonPalette, cx: &App) -> AnyElement {
    let tone = sign_color(p, profit);
    let col = moon(tone);
    let area = moon_alpha(tone, 0.12);
    div()
        .flex_1()
        .min_w(design::ui_px(cx, SPARK_MIN_W))
        .h(design::ui_px(cx, SPARK_H))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds: Bounds<Pixels>, _, window, _| {
                    let w = f32::from(bounds.size.width);
                    let h = f32::from(bounds.size.height);
                    let n = pts.len();
                    if w < 4.0 || h < 4.0 || n < 2 {
                        return;
                    }
                    let lo = pts.iter().copied().fold(0.0f32, f32::min);
                    let hi = pts.iter().copied().fold(lo, f32::max);
                    let span = (hi - lo).max(1e-6);
                    let x = |k: usize| bounds.origin.x + px(k as f32 / (n - 1) as f32 * w);
                    let y = |v: f32| bounds.origin.y + px(h - 3.0 - (v - lo) / span * (h - 6.0));
                    let base = y(lo);
                    let mut fill = PathBuilder::fill();
                    fill.move_to(gpui::point(x(0), base));
                    for (k, &v) in pts.iter().enumerate() {
                        fill.line_to(gpui::point(x(k), y(v)));
                    }
                    fill.line_to(gpui::point(x(n - 1), base));
                    if let Ok(path) = fill.build() {
                        window.paint_path(path, area);
                    }
                    let mut line = PathBuilder::stroke(px(1.5));
                    for (k, &v) in pts.iter().enumerate() {
                        let pt = gpui::point(x(k), y(v));
                        if k == 0 {
                            line.move_to(pt);
                        } else {
                            line.line_to(pt);
                        }
                    }
                    if let Ok(path) = line.build() {
                        window.paint_path(path, col);
                    }
                    let end = gpui::point(x(n - 1) - px(2.5), y(pts[n - 1]) - px(2.5));
                    window.paint_quad(
                        gpui::fill(Bounds::new(end, gpui::size(px(5.0), px(5.0))), col)
                            .corner_radii(px(2.5)),
                    );
                },
            )
            .size_full(),
        )
        .into_any_element()
}

/// One metric tile on the soft panel: the label on top (it may wrap), the value under it, an
/// optional meter, and the comparison pinned to the tile bottom so paired tiles line up.
///
/// Args:
///     metric: The metric's label, value and colour.
///     meter: Drawn under the value when present (the winrate meter).
///     delta: The caption at the tile bottom with the width its unbreakable lines need, when the
///         metric has one.
///     p: Active palette.
///     cx: App context for scaled sizes and text measurement.
///
/// Returns:
///     The tile; it takes an equal share of its pair and the pair's full height, and never gets
///     narrower than its value or a comparison line, so a narrow card wraps its tiles instead.
fn tile(
    metric: &Metric,
    meter: Option<AnyElement>,
    delta: Option<(Div, f32)>,
    p: MoonPalette,
    cx: &App,
) -> Div {
    let name = metric.name;
    let value_w = design::mono_title_text_width(cx, &metric.value, 600.0);
    let need = delta.as_ref().map_or(value_w, |d| value_w.max(d.1)).ceil() + 1.0;
    v_flex()
        .debug_selector(move || layout::kpi_tile(name))
        .flex_1()
        .min_w(px(need) + design::ui_px(cx, 22.0))
        .gap(design::ui_px(cx, 3.0))
        .px(design::ui_px(cx, 10.0))
        .py(design::ui_px(cx, 8.0))
        .bg(moon(p.panel_head))
        .border_1()
        .border_color(moon(p.border_soft))
        .rounded(design::ui_px(cx, 5.0))
        .child(
            div()
                .debug_selector(move || layout::kpi_label(name))
                .w_full()
                .min_w_0()
                .font_family(design::ui_font())
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_soft))
                .child(metric.label.clone()),
        )
        .child(
            div()
                .debug_selector(move || layout::kpi_value(name))
                .flex_none()
                .min_w(px(value_w))
                .whitespace_nowrap()
                .font_family(design::mono())
                .text_size(design::t_title(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(moon(metric.color))
                .child(metric.value.clone()),
        )
        .children(meter)
        .children(delta.map(|d| d.0.mt_auto().pt(design::ui_px(cx, 2.0))))
}

/// Winrate meter with the previous period's tick; the tick only exists when comparing.
fn meter_el(m: &KpiBand, p: MoonPalette, cx: &App) -> Option<AnyElement> {
    let wr = m.quality.winrate;
    wr.is_finite().then(|| {
        let tick = m.quality.wr_prev.filter(|v| v.is_finite()).map(|prev| {
            div()
                .id("sum-wr-prev")
                .absolute()
                .left(gpui::relative((prev / 100.0).clamp(0.0, 1.0) as f32))
                .top(design::ui_px(cx, -3.0))
                .w(px(2.0))
                .h(design::ui_px(cx, 10.0))
                .bg(moon(p.text_muted))
                .tooltip(crate::panels::common::text_tooltip(
                    t!("analytics.sum.meter_tip").to_string(),
                ))
        });
        div()
            .relative()
            .w_full()
            .child(
                MoonProgress::new("sum-wr")
                    .value(wr as f32)
                    .color(p.green)
                    .height(design::ui_value(cx, 4.0))
                    .render(),
            )
            .children(tick)
            .into_any_element()
    })
}

/// A tile's comparison caption on two lines: the coloured change first, then the muted previous
/// value or extra fact. Each part stays whole on one line; no separator dot is drawn.
/// Without a comparison the extra fact still shows, alone on a muted line.
///
/// Args:
///     name: Metric name for the debug selectors.
///     line: The comparison, when one is shown.
///     extra: A fact shown when the comparison has no previous value, or without a comparison.
///     p: Active palette.
///     cx: App context for scaled sizes.
///
/// Returns:
///     The caption with the measured width of its widest unbreakable line (0 for a muted fact,
///     which may wrap), or nothing when there is neither a comparison nor a fact.
fn delta_el(
    name: &'static str,
    line: Option<&DeltaLine>,
    extra: Option<String>,
    p: MoonPalette,
    cx: &App,
) -> Option<(Div, f32)> {
    let Some(line) = line else {
        return extra.map(|extra| (muted_line(name, extra, p, cx), 0.0));
    };
    let tail = line.prev_text.clone().or(extra);
    let width = std::iter::once(&line.text)
        .chain(tail.as_ref())
        .map(|text| design::mono_caption_text_width(cx, text, 400.0))
        .fold(0.0, f32::max);
    let part = |k: usize, text: String, color: u32| {
        div()
            .debug_selector(move || layout::kpi_delta_part(name, k))
            .whitespace_nowrap()
            .text_color(moon(color))
            .child(text)
    };
    Some((
        v_flex()
            .debug_selector(move || layout::kpi_delta(name))
            .w_full()
            .min_w_0()
            .font_family(design::mono())
            .text_size(design::t_caption(cx))
            .child(part(0, line.text.clone(), line.tone.color(p)))
            .children(tail.map(|tail| part(1, tail, p.text_soft))),
        width,
    ))
}

/// A muted caption under a metric (recovery hint, hold time without a comparison).
fn muted_line(name: &'static str, text: String, p: MoonPalette, cx: &App) -> Div {
    div()
        .debug_selector(move || layout::kpi_delta(name))
        .font_family(design::mono())
        .text_size(design::t_caption(cx))
        .text_color(moon(p.text_soft))
        .child(text)
}

#[cfg(test)]
mod tests;
