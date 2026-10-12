//! The Summary's one "Profit for the period" card: a cumulative pane (total area and line,
//! drawdown band, the shown cores' lines, peak/trough labels) above a day-bars pane, both on
//! one time axis, with one crosshair and one popup.
//!
//! The card renders from a [`ProfitChartModel`] over `&App`, so the layout probe draws it
//! without a view; hover state stays on the view (its fences must survive reloads) and reaches
//! the card only as the hovered bucket plus the [`ChartActs`] callbacks.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Bounds, Div, FontWeight, Hsla, InteractiveElement, IntoElement,
    MouseMoveEvent, ParentElement, PathBuilder, Pixels, StatefulInteractiveElement, Styled, Window,
    canvas, div, px, relative,
};
use moon_core::db::analytics::Summary;
use moon_core::util::fmt;
use moon_ui::{MoonPalette, MoonSegmentItem, MoonSegmentedControl, h_flex, v_flex};
use rust_i18n::t;

use super::charts::{
    PopupKey, bucket_label, core_color, popup_core_row, popup_overlay, popup_shell, profit_trades,
};
use super::derived::SummaryDerived;
use super::model::{chart_range, shown_cores};
use super::state::{CoreLines, SummaryUi};
use super::{fmt_signed, sign_color};
use crate::design::{self, moon, moon_alpha};

/// Cumulative pane height at the design reference size.
const TOP_H: f32 = 250.0;
/// Gap between the two panes at the design reference size.
const PANE_GAP: f32 = 18.0;
/// Day-bars pane height at the design reference size.
const BOT_H: f32 = 100.0;
/// Axis strip height under the bars at the design reference size.
const AXIS_H: f32 = 18.0;
/// Horizontal inset of the plot, so edge bars and labels stay inside the card.
const PAD_X: f32 = 12.0;
/// Share of a bucket's width a bar takes.
const BAR_FILL: f32 = 0.68;
/// At most this many axis labels besides the last one.
const MAX_TICKS: usize = 6;
/// Vertical breathing room above and below the cumulative range.
const RANGE_PAD: f32 = 0.08;

/// One shown core in the chart: its index into `core_days`, colour and curve.
#[derive(Clone)]
pub(in crate::analytics) struct ShownCore {
    /// Index into the snapshot's `core_days`.
    pub(in crate::analytics) ix: usize,
    /// Core name for the legend and popup.
    pub(in crate::analytics) name: String,
    /// Period total for the legend.
    pub(in crate::analytics) total: f64,
    /// Resolved line colour.
    pub(in crate::analytics) color: Hsla,
    /// Cumulative curve, shared with the snapshot cache.
    pub(in crate::analytics) curve: Rc<[f32]>,
}

/// Everything the profit card draws, built once per render from the snapshot.
#[derive(Clone)]
pub(in crate::analytics) struct ProfitChartModel {
    /// Card title.
    pub(in crate::analytics) title: String,
    /// Unit and grid caption under the title.
    pub(in crate::analytics) sub: String,
    /// Chart line mode, for the segmented control.
    pub(in crate::analytics) lines: CoreLines,
    /// Scope marker line and its tooltip, when the scope hides cores (J6).
    pub(in crate::analytics) marker: Option<(String, String)>,
    /// Hint shown when "selected" is on with nothing picked.
    pub(in crate::analytics) hint: Option<String>,
    /// The cores drawn, in legend order.
    pub(in crate::analytics) shown: Vec<ShownCore>,
    /// Total cumulative curve.
    pub(in crate::analytics) total: Rc<[f32]>,
    /// Exact running totals, for labels and the popup.
    pub(in crate::analytics) cum: Rc<[f64]>,
    /// Running peak of the total, the drawdown band's upper edge.
    pub(in crate::analytics) peaks: Rc<[f32]>,
    /// Lower bound of the cumulative pane, padded.
    pub(in crate::analytics) vmin: f32,
    /// Upper bound of the cumulative pane, padded.
    pub(in crate::analytics) vmax: f32,
    /// Peak bucket opening the maximum drawdown.
    pub(in crate::analytics) dd_peak_ix: Option<usize>,
    /// Bottom bucket of the maximum drawdown.
    pub(in crate::analytics) dd_trough_ix: Option<usize>,
    /// Bucket with the largest profit.
    pub(in crate::analytics) day_max_ix: Option<usize>,
    /// Bucket with the smallest profit.
    pub(in crate::analytics) day_min_ix: Option<usize>,
    /// Axis labels: bucket index and text.
    pub(in crate::analytics) ticks: Vec<(usize, String)>,
    /// The snapshot, for bar values and the popup's per-core values.
    pub(in crate::analytics) data: Arc<Summary>,
    /// Display zone of the bucket labels.
    pub(in crate::analytics) zone: chrono_tz::Tz,
}

impl ProfitChartModel {
    /// Build the card model from a snapshot, its cache and the view's display choices.
    ///
    /// Args:
    ///     derived: Snapshot derivations: curves of every core, colours, drawdown marks.
    ///     ui: Chart line mode and picked cores.
    ///     zone: Display zone for bucket labels.
    ///     marker: Scope-marker line and tooltip, when the scope hides cores.
    ///     p: Active palette, for fallback core colours.
    ///
    /// Returns:
    ///     The model; its y-range comes only from `chart_range` over the shown cores (J1, F3).
    pub(in crate::analytics) fn new(
        derived: &SummaryDerived,
        ui: &SummaryUi,
        zone: chrono_tz::Tz,
        marker: Option<(String, String)>,
        p: MoonPalette,
    ) -> Self {
        let data = derived.data().clone();
        let unit = crate::analytics::pnl_unit_label();
        let sub = match data.bucket_secs {
            b if b < 86_400 => t!("analytics.sum.chart.sub_hours", unit = unit),
            86_400 => t!("analytics.sum.chart.sub_days", unit = unit),
            _ => t!("analytics.sum.chart.sub_weeks", unit = unit),
        }
        .to_string();
        let shown_ix = shown_cores(&data.core_days, ui.lines, &ui.picked);
        let (lo, hi) = chart_range(&derived.pts, &derived.core_curves, &shown_ix);
        let pad = (hi - lo) * RANGE_PAD;
        let shown = shown_ix
            .iter()
            .map(|&ix| ShownCore {
                ix,
                name: data.core_days[ix].name.clone(),
                total: data.core_days[ix].total,
                color: core_color(&derived.colors, ix, p),
                curve: derived.core_curves[ix].clone(),
            })
            .collect();
        let hint = super::model::selection_hint_due(ui.lines, &shown_ix)
            .then(|| t!("analytics.sum.chart.lines_sel_hint").to_string());
        let ticks = tick_indices(data.days.len())
            .into_iter()
            .map(|k| (k, bucket_label(data.days[k].start, data.bucket_secs, zone)))
            .collect();
        Self {
            title: t!("analytics.sum.chart.title").to_string(),
            sub,
            lines: ui.lines,
            marker,
            hint,
            shown,
            total: derived.pts.clone(),
            cum: derived.cum.clone(),
            peaks: derived.peaks.clone(),
            vmin: lo - pad,
            vmax: hi + pad,
            dd_peak_ix: derived.dd_peak_ix,
            dd_trough_ix: derived.dd_trough_ix,
            day_max_ix: derived.day_max_ix,
            day_min_ix: derived.day_min_ix,
            ticks,
            data,
            zone,
        }
    }

    /// Buckets on the time axis.
    pub(in crate::analytics) fn n(&self) -> usize {
        self.data.days.len()
    }
}

/// Axis tick buckets: every k-th bucket from the first, plus the last; a regular tick too close
/// to the last is dropped so the two labels never collide.
pub(in crate::analytics) fn tick_indices(n: usize) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let step = n.div_ceil(MAX_TICKS).max(1);
    let mut out: Vec<usize> = (0..n).step_by(step).collect();
    if let Some(&last) = out.last()
        && last != n - 1
        && (n - 1 - last) * 2 < step
    {
        out.pop();
    }
    if out.last() != Some(&(n - 1)) {
        out.push(n - 1);
    }
    out
}

/// Horizontal position of bucket `bi` across the plot, the one mapping both panes share.
pub(in crate::analytics) fn bucket_frac(bi: usize, n: usize) -> f32 {
    if n > 1 {
        bi as f32 / (n - 1) as f32
    } else {
        0.0
    }
}

/// Exact normalized span of a hover column, including f32 rounding and clipped ends.
fn hover_column(bi: usize, n: usize) -> (f32, f32) {
    let step = 1.0 / n.saturating_sub(1).max(1) as f32;
    let centre = bi as f32 * step;
    (
        (centre - step / 2.0).max(0.0),
        (centre + step / 2.0).min(1.0),
    )
}

/// Find the highest half-open column containing the pointer; uncovered regions return `None`.
pub(in crate::analytics) fn hover_bucket_at(frac: f32, n: usize) -> Option<usize> {
    if !frac.is_finite() || !(0.0..=1.0).contains(&frac) {
        return None;
    }
    // Left edges are monotonic; the last matching left edge keeps topmost ties.
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if hover_column(mid, n).0 <= frac {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let bi = lo.checked_sub(1)?;
    (frac < hover_column(bi, n).1).then_some(bi)
}

/// One shown core's line in the popup.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::analytics) struct PopupCore {
    /// Core name.
    pub(in crate::analytics) name: String,
    /// Line colour, as the dot.
    pub(in crate::analytics) dot: Hsla,
    /// The core's profit in this bucket.
    pub(in crate::analytics) value: f64,
    /// The core's trades in this bucket.
    pub(in crate::analytics) trades: i64,
}

/// What the crosshair popup lists for one bucket (J2).
#[derive(Clone, Debug, PartialEq)]
pub(in crate::analytics) struct PopupModel {
    /// Bucket date or hour.
    pub(in crate::analytics) date: String,
    /// Profit of the bucket.
    pub(in crate::analytics) day: f64,
    /// Running total through the bucket.
    pub(in crate::analytics) cum: f64,
    /// Trades in the bucket.
    pub(in crate::analytics) trades: i64,
    /// Every shown core, no cap.
    pub(in crate::analytics) cores: Vec<PopupCore>,
}

/// Build the popup of bucket `bi`, or `None` past the end.
pub(in crate::analytics) fn popup_model(m: &ProfitChartModel, bi: usize) -> Option<PopupModel> {
    let day = m.data.days.get(bi)?;
    Some(PopupModel {
        date: bucket_label(day.start, m.data.bucket_secs, m.zone),
        day: day.profit,
        cum: m.cum.get(bi).copied().unwrap_or(0.0),
        trades: day.trades,
        cores: m
            .shown
            .iter()
            .map(|c| {
                let core = &m.data.core_days[c.ix];
                PopupCore {
                    name: c.name.clone(),
                    dot: c.color,
                    value: core.per_bucket.get(bi).copied().unwrap_or(0.0),
                    trades: core.per_bucket_trades.get(bi).copied().unwrap_or(0),
                }
            })
            .collect(),
    })
}

/// Callbacks the card raises; the view owns what they change.
#[derive(Clone)]
pub(in crate::analytics) struct ChartActs {
    /// The line-mode control was clicked.
    pub(in crate::analytics) on_lines: Rc<dyn Fn(CoreLines, &mut Window, &mut App)>,
    /// The pointer entered (`true`) or left (`false`) the plot.
    pub(in crate::analytics) on_area: Rc<dyn Fn(bool, &mut Window, &mut App)>,
    /// The pointer moved onto a bucket, or into an uncovered strip (`None`).
    pub(in crate::analytics) on_bucket: Rc<dyn Fn(Option<usize>, &mut Window, &mut App)>,
    /// The pointer entered or left the popup of a bucket.
    pub(in crate::analytics) on_popup: Rc<dyn Fn(usize, bool, &mut Window, &mut App)>,
}

/// Render the profit card.
///
/// Args:
///     m: Card model.
///     hover: Bucket under the crosshair, if any.
///     acts: Callbacks into the view.
///     p: Active palette.
///     cx: App context for scaled sizes and text measurement.
///
/// Returns:
///     The card; it schedules nothing.
pub(in crate::analytics) fn profit_chart_el(
    m: &ProfitChartModel,
    hover: Option<usize>,
    acts: &ChartActs,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    v_flex()
        .debug_selector(|| "sum-chart-card".into())
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 8.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 10.0))
        .rounded(design::ui_px(cx, 8.0))
        .bg(moon(p.panel))
        .border_1()
        .border_color(moon(p.border))
        .child(header_el(m, acts, p, cx))
        .child(legend_el(m, p, cx))
        .child(plot_el(m, hover, acts, p, cx))
        .into_any_element()
}

/// Title, unit caption, scope marker, and the line-mode control on the right.
fn header_el(m: &ProfitChartModel, acts: &ChartActs, p: MoonPalette, cx: &App) -> Div {
    let on_lines = acts.on_lines.clone();
    let items = [
        (CoreLines::None, "analytics.sum.chart.lines_none"),
        (CoreLines::Top5, "analytics.sum.chart.lines_top"),
        (CoreLines::Selected, "analytics.sum.chart.lines_sel"),
    ]
    .map(|(mode, key)| {
        MoonSegmentItem::new("", t!(key).to_string())
            .fit_width(cx, 40.0, 120.0)
            .selected(m.lines == mode)
    });
    let seg = MoonSegmentedControl::new("an-sum-lines")
        .items(items)
        .on_click(move |ix, _, window, app| {
            let mode = match ix {
                0 => CoreLines::None,
                1 => CoreLines::Top5,
                _ => CoreLines::Selected,
            };
            on_lines(mode, window, app);
        });
    h_flex()
        .w_full()
        .flex_wrap()
        .items_center()
        .gap_x(design::ui_px(cx, 10.0))
        .gap_y(design::ui_px(cx, 4.0))
        .child(
            div()
                .debug_selector(|| "sum-chart-title".into())
                .flex_none()
                .font_family(design::ui_font())
                .text_size(design::t_title(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .child(m.title.clone()),
        )
        .child(
            div()
                .flex_none()
                .font_family(design::ui_font())
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(m.sub.clone()),
        )
        .children(m.marker.clone().map(|(line, tip)| {
            div()
                .id("an-summary-scope-marker")
                .debug_selector(|| "sum-chart-marker".into())
                .min_w_0()
                .truncate()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .tooltip(crate::panels::common::text_tooltip(tip))
                .child(line)
        }))
        .child(
            h_flex()
                .debug_selector(|| "sum-chart-lines".into())
                .ml_auto()
                .flex_none()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(
                    div()
                        .flex_none()
                        .font_family(design::ui_font())
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(t!("analytics.sum.chart.lines").to_string()),
                )
                .child(seg),
        )
}

/// Wrapping legend: total, each shown core with its total, the drawdown swatch, and the hint.
fn legend_el(m: &ProfitChartModel, p: MoonPalette, cx: &App) -> Div {
    let total = m.cum.last().copied().unwrap_or(0.0);
    // An entry never grows past the card: its name shrinks to an ellipsis with the full name in a
    // tooltip, while the swatch and the value keep their whole width.
    let item = |i: usize, swatch: Div, text: String, value: Option<(String, u32)>| {
        h_flex()
            .debug_selector(move || format!("sum-chart-legend-{i}"))
            .flex_none()
            .max_w_full()
            .min_w_0()
            .items_center()
            .gap(design::ui_px(cx, 5.0))
            .child(swatch)
            .child(
                div()
                    .id(("an-sum-legend-name", i))
                    .debug_selector(move || format!("sum-chart-legend-name-{i}"))
                    .min_w_0()
                    .truncate()
                    .text_color(moon(p.text_soft))
                    .tooltip(crate::panels::common::text_tooltip_value(text.clone()))
                    .child(text),
            )
            .children(value.map(|(v, tone)| {
                div()
                    .debug_selector(move || format!("sum-chart-legend-value-{i}"))
                    .flex_none()
                    .text_color(moon(tone))
                    .child(v)
            }))
    };
    let dot = |c: Hsla| {
        div()
            .flex_none()
            .w(design::ui_px(cx, 10.0))
            .h(design::ui_px(cx, 3.0))
            .rounded(px(1.5))
            .bg(c)
    };
    let mut row = h_flex()
        .w_full()
        .flex_wrap()
        .gap_x(design::ui_px(cx, 14.0))
        .gap_y(design::ui_px(cx, 4.0))
        .font_family(design::mono())
        .text_size(design::t_caption(cx))
        .child(item(
            0,
            dot(moon(sign_color(p, total))),
            t!("analytics.sum.chart.total", v = fmt_signed(total)).to_string(),
            None,
        ));
    for (k, core) in m.shown.iter().enumerate() {
        row = row.child(item(
            k + 1,
            dot(core.color),
            core.name.clone(),
            Some((fmt_signed(core.total), sign_color(p, core.total))),
        ));
    }
    row = row.child(item(
        m.shown.len() + 1,
        div()
            .flex_none()
            .w(design::ui_px(cx, 10.0))
            .h(design::ui_px(cx, 8.0))
            .bg(moon_alpha(p.orange, 0.3)),
        t!("analytics.sum.chart.drawdown").to_string(),
        None,
    ));
    row.children(m.hint.clone().map(|hint| {
        div()
            .flex_none()
            .font_family(design::ui_font())
            .text_color(moon(p.text_muted))
            .child(hint)
    }))
}

/// Drawdown band alpha: stronger on dark surfaces so it still reads there.
fn band_alpha(p: MoonPalette) -> f32 {
    if p.is_light() { 0.16 } else { 0.22 }
}

/// Top of a bar-extreme label, placed so it never covers its own bar.
///
/// The label sits past the bar's tip when it fits there; when the bar reaches the edge of its
/// half of the pane (the worst day always does), it moves to the other side of the zero line,
/// where its own bucket has no bar.
///
/// Args:
///     above: True for the best-day label (bar grows up), false for the worst day.
///     tip_y: Y of the bar's far end.
///     zero_y: Y of the zero line.
///     min_y: Highest top the label may take.
///     max_bottom: Lowest bottom the label may reach (the axis strip starts there).
///     cap_h: Label height.
///
/// Returns:
///     The label's top, in plot coordinates.
fn extreme_label_y(
    above: bool,
    tip_y: f32,
    zero_y: f32,
    min_y: f32,
    max_bottom: f32,
    cap_h: f32,
) -> f32 {
    const GAP: f32 = 2.0;
    if above {
        let y = tip_y - cap_h - GAP;
        if y >= min_y {
            y
        } else {
            (zero_y + GAP).min(max_bottom - cap_h)
        }
    } else {
        let y = tip_y + GAP;
        if y + cap_h <= max_bottom {
            y
        } else {
            (zero_y - cap_h - GAP).max(min_y)
        }
    }
}

/// The two panes, their labels, the axis, the hover surface and the popup.
fn plot_el(
    m: &ProfitChartModel,
    hover: Option<usize>,
    acts: &ChartActs,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let n = m.n();
    let top_h = design::ui_value(cx, TOP_H);
    let gap = design::ui_value(cx, PANE_GAP);
    let bot_h = design::ui_value(cx, BOT_H);
    let axis_h = design::ui_value(cx, AXIS_H);
    let pad = design::ui_value(cx, PAD_X);
    let span = (m.vmax - m.vmin).max(1e-6);
    let vmax = m.vmax;
    let y_top = move |v: f32| (vmax - v) / span * top_h;
    let profits: Vec<f64> = m.data.days.iter().map(|d| d.profit).collect();
    let bar_max = profits
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(0.0f64, |a, v| a.max(v.abs()))
        .max(1e-9);
    let zero_y = top_h + gap + bot_h / 2.0;
    let bar_y = move |v: f64| zero_y - (v / bar_max) as f32 * (bot_h / 2.0);

    // Absolutely placed mono labels inside the inset plot; edge labels hug their side.
    let cap_h = f32::from(design::t_caption(cx)) * 1.4;
    // Measured with the weight it is drawn with, so centring and edge hugging use the real width.
    let label = |frac: f32, y: f32, text: String, color: u32, sel: String, weight: FontWeight| {
        let w = design::mono_caption_text_width(cx, &text, weight.0);
        let el = div()
            .debug_selector(move || sel)
            .absolute()
            .top(px(y))
            .flex_none()
            .whitespace_nowrap()
            .font_family(design::mono())
            .font_weight(weight)
            .text_size(design::t_caption(cx))
            // The box is exactly the height the placement math reserves for it.
            .line_height(px(cap_h))
            .text_color(moon(color))
            .child(text);
        if frac < 0.12 {
            el.left(px(0.0))
        } else if frac > 0.88 {
            el.right(px(0.0))
        } else {
            el.left(relative(frac)).ml(px(-w / 2.0))
        }
    };
    let mut labels: Vec<Div> = Vec::new();
    if let Some(k) = m.dd_peak_ix.filter(|&k| k < n) {
        let v = m.cum[k];
        labels.push(label(
            bucket_frac(k, n),
            (y_top(m.total[k]) - cap_h - 2.0).max(0.0),
            fmt_signed(v),
            sign_color(p, v),
            "sum-chart-peak".into(),
            FontWeight::SEMIBOLD,
        ));
    }
    if let Some(k) = m.dd_trough_ix.filter(|&k| k < n) {
        let v = m.cum[k];
        labels.push(label(
            bucket_frac(k, n),
            (y_top(m.total[k]) + 4.0).min(top_h - cap_h),
            fmt_signed(v),
            sign_color(p, v),
            "sum-chart-trough".into(),
            FontWeight::SEMIBOLD,
        ));
    }
    for (k, above) in [(m.day_max_ix, true), (m.day_min_ix, false)]
        .into_iter()
        .filter_map(|(k, above)| k.filter(|&k| k < n).map(|k| (k, above)))
    {
        let v = profits[k];
        if (above && v <= 0.0) || (!above && v >= 0.0) {
            continue;
        }
        let y = extreme_label_y(
            above,
            bar_y(v),
            zero_y,
            top_h + gap - cap_h,
            top_h + gap + bot_h,
            cap_h,
        );
        labels.push(label(
            bucket_frac(k, n),
            y,
            fmt_signed(v),
            sign_color(p, v),
            format!("sum-chart-bar-{}", if above { "max" } else { "min" }),
            FontWeight::SEMIBOLD,
        ));
    }
    for (i, (k, text)) in m.ticks.iter().enumerate() {
        let frac = bucket_frac(*k, n);
        let el = label(
            frac,
            top_h + gap + bot_h + 2.0,
            text.clone(),
            p.text_muted,
            format!("sum-chart-tick-{i}"),
            FontWeight::NORMAL,
        );
        labels.push(el);
    }

    let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
    let prepaint = bounds.clone();
    let paint = PaintData {
        total: m.total.clone(),
        peaks: m.peaks.clone(),
        curves: m.shown.iter().map(|c| (c.curve.clone(), c.color)).collect(),
        profits: profits.clone().into(),
        vmin: m.vmin,
        vmax: m.vmax,
        bar_max,
        top_h,
        gap,
        bot_h,
        hover,
        tone: sign_color(p, m.cum.last().copied().unwrap_or(0.0)),
        band: moon_alpha(p.orange, band_alpha(p)),
        grid: moon_alpha(p.text_muted, 0.14),
        zero: moon_alpha(p.text_muted, 0.5),
        green: p.green,
        orange: p.orange,
        cross: moon_alpha(p.text_muted, 0.8),
    };
    let chart = canvas(
        move |b, _, _| prepaint.set(b),
        move |b, _, window, _| paint.paint(b, window),
    )
    .absolute()
    .left_0()
    .top_0()
    .w_full()
    .h(px(top_h + gap + bot_h));

    let (on_area, on_bucket) = (acts.on_area.clone(), acts.on_bucket.clone());
    let surface = div()
        .id("an-sum-chart-hover")
        .absolute()
        .left_0()
        .top_0()
        .w_full()
        .h(px(top_h + gap + bot_h))
        .on_hover(move |hovered, window, cx| on_area(*hovered, window, cx))
        .on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
            // A drag must not reopen the source popup, matching GPUI's own hover gate.
            if cx.has_active_drag() {
                return;
            }
            let b = bounds.get();
            let width = f32::from(b.size.width);
            if width <= 0.0 {
                return;
            }
            let frac = f32::from(event.position.x - b.origin.x) / width;
            on_bucket(hover_bucket_at(frac, n), window, cx);
        });

    let popup = hover.and_then(|bi| popup_model(m, bi)).map(|pm| {
        let bi = hover.unwrap_or_default();
        let on_popup = acts.on_popup.clone();
        let (card, w) = popup_card_el(&pm, p, cx);
        popup_overlay(
            card,
            w,
            bucket_frac(bi, n),
            PopupKey::Profit(bi),
            Rc::new(move |hovered, window, cx| on_popup(bi, hovered, window, cx)),
            p,
            cx,
        )
    });

    div()
        .debug_selector(|| "sum-chart-plot".into())
        .w_full()
        .px(px(pad))
        .child(
            div()
                .relative()
                .w_full()
                .h(px(top_h + gap + bot_h + axis_h))
                .child(chart)
                .children(labels)
                .child(surface)
                .children(popup),
        )
        .into_any_element()
}

/// The popup card for one bucket: header, day, cumulative, trades, then every shown core.
fn popup_card_el(pm: &PopupModel, p: MoonPalette, cx: &App) -> (Div, f32) {
    let w = |s: &str| design::mono_caption_text_width(cx, s, 400.0);
    let line = |label: String, value: String, color: u32| {
        h_flex()
            .justify_between()
            .gap(design::ui_px(cx, 10.0))
            .child(div().text_color(moon(p.text_soft)).child(label))
            .child(div().text_color(moon(color)).child(value))
    };
    let day = t!("analytics.sum.chart.pop_day").to_string();
    let cum = t!("analytics.sum.chart.pop_cum").to_string();
    let trades = t!("analytics.sum.chart.pop_trades").to_string();
    let gap = f32::from(design::ui_px(cx, 10.0));
    let row_chrome = f32::from(design::ui_px(cx, 6.0)) + f32::from(design::ui_px(cx, 5.0)) * 2.0;
    let content_w = [
        w(&pm.date),
        w(&day) + gap + w(&fmt_signed(pm.day)),
        w(&cum) + gap + w(&fmt_signed(pm.cum)),
        w(&trades) + gap + w(&fmt::group_thousands(&pm.trades.to_string())),
    ]
    .into_iter()
    .chain(
        pm.cores
            .iter()
            .map(|c| row_chrome + w(&c.name) + w(&profit_trades(c.value, c.trades))),
    )
    .fold(0.0, f32::max);
    let mut card = popup_shell(p, cx)
        .debug_selector(|| "sum-chart-popup".into())
        .font_family(design::mono())
        .child(div().text_color(moon(p.text)).child(pm.date.clone()))
        .child(line(day, fmt_signed(pm.day), sign_color(p, pm.day)))
        .child(line(cum, fmt_signed(pm.cum), sign_color(p, pm.cum)))
        .child(line(
            trades,
            fmt::group_thousands(&pm.trades.to_string()),
            p.text,
        ));
    for c in &pm.cores {
        card = card.child(popup_core_row(
            c.name.clone(),
            c.dot,
            c.value,
            c.trades,
            p,
            cx,
        ));
    }
    (card, content_w)
}

/// Everything the paint closure needs, owned so the closure is `'static`.
struct PaintData {
    total: Rc<[f32]>,
    peaks: Rc<[f32]>,
    curves: Vec<(Rc<[f32]>, Hsla)>,
    profits: Rc<[f64]>,
    vmin: f32,
    vmax: f32,
    bar_max: f64,
    top_h: f32,
    gap: f32,
    bot_h: f32,
    hover: Option<usize>,
    tone: u32,
    band: Hsla,
    grid: Hsla,
    zero: Hsla,
    green: u32,
    orange: u32,
    cross: Hsla,
}

impl PaintData {
    /// Paint both panes and the crosshair into `b`.
    fn paint(&self, b: Bounds<Pixels>, window: &mut Window) {
        let w = f32::from(b.size.width);
        let n = self.total.len();
        if w < 4.0 || n == 0 {
            return;
        }
        let span = (self.vmax - self.vmin).max(1e-6);
        let x = |k: usize| b.origin.x + px(bucket_frac(k, n) * w);
        let y = |v: f32| b.origin.y + px((self.vmax - v) / span * self.top_h);
        let rule = |yy: Pixels, color: Hsla, window: &mut Window| {
            window.paint_quad(gpui::fill(
                Bounds::new(gpui::point(b.origin.x, yy), gpui::size(px(w), px(1.0))),
                color,
            ));
        };
        for i in 0..=4 {
            rule(
                b.origin.y + px(self.top_h * i as f32 / 4.0),
                self.grid,
                window,
            );
        }
        if self.vmin < 0.0 && self.vmax > 0.0 {
            let zy = y(0.0);
            let mut xx = 0.0;
            while xx < w {
                window.paint_quad(gpui::fill(
                    Bounds::new(
                        gpui::point(b.origin.x + px(xx), zy),
                        gpui::size(px(4.0_f32.min(w - xx)), px(1.0)),
                    ),
                    self.zero,
                ));
                xx += 8.0;
            }
        }
        if n > 1 {
            // Drawdown band: along the running peak, back along the curve.
            let mut band = PathBuilder::fill();
            band.move_to(gpui::point(x(0), y(self.peaks[0])));
            for k in 1..n {
                band.line_to(gpui::point(x(k), y(self.peaks[k])));
            }
            for k in (0..n).rev() {
                band.line_to(gpui::point(x(k), y(self.total[k])));
            }
            if let Ok(path) = band.build() {
                window.paint_path(path, self.band);
            }
            for (curve, color) in &self.curves {
                let mut pb = PathBuilder::stroke(px(1.4));
                for (k, &v) in curve.iter().take(n).enumerate() {
                    let pt = gpui::point(x(k), y(v));
                    if k == 0 {
                        pb.move_to(pt);
                    } else {
                        pb.line_to(pt);
                    }
                }
                if let Ok(path) = pb.build() {
                    window.paint_path(path, *color);
                }
            }
            let base = y(self.vmin.max(0.0).min(self.vmax));
            let mut area = PathBuilder::fill();
            area.move_to(gpui::point(x(0), base));
            for (k, &v) in self.total.iter().enumerate() {
                area.line_to(gpui::point(x(k), y(v)));
            }
            area.line_to(gpui::point(x(n - 1), base));
            if let Ok(path) = area.build() {
                window.paint_path(path, moon_alpha(self.tone, 0.10));
            }
            let mut line = PathBuilder::stroke(px(2.4));
            for (k, &v) in self.total.iter().enumerate() {
                let pt = gpui::point(x(k), y(v));
                if k == 0 {
                    line.move_to(pt);
                } else {
                    line.line_to(pt);
                }
            }
            if let Ok(path) = line.build() {
                window.paint_path(path, moon(self.tone));
            }
        }
        let end = gpui::point(x(n - 1) - px(3.0), y(self.total[n - 1]) - px(3.0));
        window.paint_quad(
            gpui::fill(
                Bounds::new(end, gpui::size(px(6.0), px(6.0))),
                moon(self.tone),
            )
            .corner_radii(px(3.0)),
        );
        // Day bars on the same x mapping, around the bottom pane's zero line.
        let zero_y = self.top_h + self.gap + self.bot_h / 2.0;
        let bw = (w / n as f32 * BAR_FILL).clamp(1.0, 24.0);
        for (k, &v) in self.profits.iter().enumerate() {
            if !v.is_finite() || v == 0.0 {
                continue;
            }
            let h = ((v.abs() / self.bar_max) as f32 * (self.bot_h / 2.0)).max(1.0);
            let top = if v > 0.0 { zero_y - h } else { zero_y };
            let left = (f32::from(x(k) - b.origin.x) - bw / 2.0).clamp(0.0, (w - bw).max(0.0));
            window.paint_quad(gpui::fill(
                Bounds::new(
                    gpui::point(b.origin.x + px(left), b.origin.y + px(top)),
                    gpui::size(px(bw), px(h)),
                ),
                moon(if v > 0.0 { self.green } else { self.orange }),
            ));
        }
        rule(b.origin.y + px(zero_y), self.grid, window);
        if let Some(bi) = self.hover.filter(|&bi| bi < n) {
            let cx_ = x(bi).min(b.origin.x + px(w - 1.0));
            let mut yy = 0.0;
            let full = self.top_h + self.gap + self.bot_h;
            while yy < full {
                window.paint_quad(gpui::fill(
                    Bounds::new(
                        gpui::point(cx_, b.origin.y + px(yy)),
                        gpui::size(px(1.0), px(3.0_f32.min(full - yy))),
                    ),
                    self.cross,
                ));
                yy += 6.0;
            }
        }
    }
}

#[cfg(test)]
mod tests;
