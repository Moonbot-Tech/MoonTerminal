//! "Summary" tab of the "Analytics" window: KPI cards compared against the
//! previous period, charts (cumulative profit + daily bars), the top 5
//! best/worst trades and automatic "insights". Layout follows the
//! analytics-mock artifact.

use gpui::*;
use moon_ui::{
    MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonPalette, MoonSegmentItem, MoonSegmentedControl,
    MoonTone, h_flex, v_flex,
};
use rust_i18n::t;

use super::AnalyticsView;
mod charts;
mod cumulative;
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::{Summary, TopTrade};
use moon_core::util::fmt::{self, compact};

/// Format a compact signed value such as `+341.2`, `-40.1`, or `0` — WITHOUT any unit.
///
/// For dimensionless figures like profit factor. Rounding happens before sign selection, so a
/// value that rounds to zero has no minus sign. Returns an em dash when the input or rounded
/// result is non-finite.
pub(in crate::analytics) fn fmt_signed_plain(v: f64) -> String {
    let Some(v) = fmt::round_to(v, 2) else {
        return "—".to_string();
    };
    if v > 0.0 {
        format!("+{}", compact(v, 2))
    } else {
        compact(v, 2)
    }
}

/// Format a compact signed PROFIT, carrying the active metric's unit: "%" in percent mode
/// (the report `Profit` column), nothing in quote-money mode. Every profit figure on the window goes
/// through here, so the unit follows the toolbar switch everywhere at once. The em dash for a
/// non-finite value is never given a unit.
///
/// Args:
///     v: Profit value in the active comparable unit.
///
/// Returns:
///     Signed compact value with `%` only in Percent mode, or an em dash.
pub(super) fn fmt_signed(v: f64) -> String {
    let s = fmt_signed_plain(v);
    if s == "—" {
        s
    } else {
        format!("{}{}", s, super::pnl_suffix())
    }
}

/// Signed profit carrying the FULL unit for prose (the insights sentences): "+15.34%" in percent
/// mode, "+15.34 USDC" in a USDC scope. [`fmt_signed`] leaves a money figure unitless because its
/// surrounding label supplies the exact quote ticker; a free-standing sentence has no such label,
/// so the ticker rides along here. A non-finite value stays a bare em dash.
///
/// Args:
///     v: Profit value in the active comparable unit.
///
/// Returns:
///     Signed compact value with exact ticker or `%`, or an em dash.
pub(super) fn fmt_signed_unit(v: f64) -> String {
    let s = fmt_signed(v);
    if s == "—" || super::pnl_is_pct() {
        s
    } else {
        let unit = super::pnl_unit_label();
        if unit.is_empty() {
            s
        } else {
            format!("{s} {unit}")
        }
    }
}

/// Return the terminal colour for the same rounded sign used by [`fmt_signed`].
///
/// Positive values are green, negative values are orange, and zero or a non-finite rounding result
/// is muted. This keeps the colour consistent with the displayed text or em dash.
pub(super) fn sign_color(p: MoonPalette, v: f64) -> u32 {
    match fmt::round_to(v, 2) {
        Some(v) if v > 0.0 => p.green,
        Some(v) if v < 0.0 => p.orange,
        _ => p.text_muted,
    }
}

impl AnalyticsView {
    /// Render summary data or the placeholder dictated by its exhaustive load state.
    pub(super) fn summary_tab(&self, p: MoonPalette, cx: &Context<Self>) -> AnyElement {
        let data = match self.data.view(|d| d.cur.n == 0) {
            Ok(d) => d.clone(),
            Err(note) => return super::note_el("an-summary-note", note, 18.0, p, cx),
        };
        // Core series colors come from the server's SETTINGS (ServerConfig.color,
        // as in the core selector); the fallback palette is only for cores with
        // no config entry.
        let core_colors: Vec<Hsla> = {
            let b = self.backend.read(cx);
            data.core_days
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    b.config
                        .servers
                        .iter()
                        .find(|s| s.id == c.uid)
                        .map(|s| {
                            Hsla::from(gpui::Rgba {
                                r: s.color[0] as f32 / 255.0,
                                g: s.color[1] as f32 / 255.0,
                                b: s.color[2] as f32 / 255.0,
                                a: 1.0,
                            })
                        })
                        .unwrap_or_else(|| moon(charts::fallback_core_color(p, i)))
                })
                .collect()
        };
        // The top part (KPI/charts/tops) scrolls on its own; the "Profit by
        // core" chart is PINNED to the bottom edge of the window (like the
        // bottom bar of "Strategies").
        let top = v_flex()
            .w_full()
            .p(design::ui_px(cx, 10.0))
            .gap(design::ui_px(cx, 8.0))
            .child(self.kpi_row(&data, p, cx))
            .child(
                h_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 8.0))
                    .items_start()
                    .child({
                        // Top left: the total cumulative curve with the per-core curves
                        // drawn inside it — always, there is no mode switch. The period
                        // total moved into the header slot the old checkbox occupied, so
                        // the axis row below the chart is free for the date ticks.
                        let total: f64 = data.days.iter().map(|d| d.profit).sum();
                        let shown = data.core_days.len().min(cumulative::MAX_CORE_LINES);
                        // Workspace scope marker: whether the active Auto preset itself hid a
                        // core from this window's read, distinct from the line-cap fact above,
                        // which only names how many of the (already scoped) cores got a curve.
                        let marker = self
                            .workspace_scope
                            .as_ref()
                            .map(super::AnalyticsWorkspaceScope::scope_marker);
                        let marker_facts = marker.as_ref().map_or_else(Vec::new, |m| m.facts());
                        let head = h_flex()
                            .gap(design::ui_px(cx, 8.0))
                            .items_center()
                            // The line cap is never silent: say so when it bites.
                            .children((data.core_days.len() > shown).then(|| {
                                div()
                                    .text_size(design::t_caption(cx))
                                    .text_color(moon(p.text_muted))
                                    .child(
                                        t!(
                                            "analytics.cores_shown",
                                            n = shown,
                                            total = data.core_days.len()
                                        )
                                        .to_string(),
                                    )
                            }))
                            .children((!marker_facts.is_empty()).then(|| {
                                let text = marker_facts.join(" ");
                                // Built from the SAME `Vec` the caption renders, per decision 1.
                                let tip = marker
                                    .as_ref()
                                    .map(|m| m.tooltip(std::slice::from_ref(&text)))
                                    .unwrap_or_default();
                                div()
                                    .id("an-summary-scope-marker")
                                    .text_size(design::t_caption(cx))
                                    .text_color(moon(p.text_muted))
                                    .tooltip(crate::panels::common::text_tooltip(tip))
                                    .child(text)
                            }))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(moon(sign_color(p, total)))
                                    .child(fmt_signed(total)),
                            )
                            .into_any_element();
                        chart_card_ex(
                            t!("analytics.cum_title").to_string(),
                            // The subtitle names the grid the DB actually chose: on a
                            // single-day period the curve is hourly, not per day.
                            if data.bucket_secs < 86_400 {
                                t!(
                                    "analytics.cum_sub_hours",
                                    unit = crate::analytics::pnl_unit_label()
                                )
                                .to_string()
                            } else {
                                t!(
                                    "analytics.cum_sub",
                                    unit = crate::analytics::pnl_unit_label()
                                )
                                .to_string()
                            },
                            Some(head),
                            cumulative::cumulative_area(
                                &data.days,
                                &data.core_days,
                                &core_colors,
                                self.hover_cum_bucket,
                                data.bucket_secs,
                                self.bound_zone(),
                                p,
                                cx,
                            ),
                            p,
                            cx,
                        )
                    })
                    .child({
                        // On a single day the per-day series is ONE bar, so this card
                        // switches dimension: profit per strategy type, with the cores
                        // behind each type in its popup. The AUTHORITATIVE signal is the
                        // grid the DB layer chose — an hourly bucket IS "a single day";
                        // `kinds` can be empty simply because nothing traded.
                        let by_kind = data.bucket_secs < 86_400;
                        let (title, sub, body) = if by_kind {
                            (
                                t!("analytics.kinds_title").to_string(),
                                t!(
                                    "analytics.kinds_sub",
                                    unit = crate::analytics::pnl_unit_label()
                                )
                                .to_string(),
                                charts::kind_bars(
                                    &data.kinds,
                                    &data.core_days,
                                    &core_colors,
                                    self.hover_kind,
                                    p,
                                    cx,
                                ),
                            )
                        } else {
                            (
                                t!("analytics.daily_title").to_string(),
                                t!(
                                    "analytics.daily_sub",
                                    unit = crate::analytics::pnl_unit_label()
                                )
                                .to_string(),
                                charts::daily_bars(
                                    &data.days,
                                    &data.core_days,
                                    &core_colors,
                                    self.hover_daily_bucket,
                                    data.bucket_secs,
                                    self.bound_zone(),
                                    p,
                                    cx,
                                ),
                            )
                        };
                        chart_card(title, sub, body, p, cx)
                    }),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 8.0))
                    .items_stretch()
                    .child(top_card(
                        t!("analytics.best_trades").to_string(),
                        &data.best,
                        self.bound_zone(),
                        p,
                        cx,
                    ))
                    .child(top_card(
                        t!("analytics.worst_trades").to_string(),
                        &data.worst,
                        self.bound_zone(),
                        p,
                        cx,
                    ))
                    .child(insights_card(&data, p, cx)),
            );
        v_flex()
            .size_full()
            // The top is its natural height; when space runs short it shrinks
            // and scrolls inside (basis auto + min_h_0 + overflow).
            .child(
                // Default flex (grow 0, shrink 1, basis auto): natural height,
                // shrinking and scrolling when space runs short.
                div()
                    .id("an-sum-scroll")
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .child(top),
            )
            // The bottom ranking is ELASTIC and pinned to the bottom of the window. Both its
            // overview and complete modes virtualize their rows, so the 170 px minimum remains
            // usable instead of clipping a fixed ten-row column.
            .child(
                div()
                    .flex_1()
                    .min_h(design::ui_px(cx, 170.0))
                    .w_full()
                    .px(design::ui_px(cx, 10.0))
                    .pb(design::ui_px(cx, 10.0))
                    .child({
                        let stats = charts::core_rank_stats(&data.core_days);
                        let subtitle = if let Some(share) = stats.leader_share_pct {
                            t!(
                                "analytics.cores_stats_leader",
                                total = stats.total,
                                profitable = stats.profitable,
                                losing = stats.losing,
                                share = compact(share, 0)
                            )
                            .to_string()
                        } else {
                            t!(
                                "analytics.cores_stats",
                                total = stats.total,
                                profitable = stats.profitable,
                                losing = stats.losing
                            )
                            .to_string()
                        };
                        let show_all = self.show_all_core_ranks;
                        let view = cx.entity();
                        let modes = MoonSegmentedControl::new("an-core-rank-mode")
                            .items([
                                MoonSegmentItem::new(
                                    "",
                                    t!("analytics.cores_overview").to_string(),
                                )
                                .fit_width(cx, 58.0, 92.0)
                                .selected(!show_all),
                                MoonSegmentItem::new("", t!("analytics.cores_all").to_string())
                                    .fit_width(cx, 44.0, 72.0)
                                    .selected(show_all),
                            ])
                            .on_click(move |ix, _, _, app| {
                                let next = ix == 1;
                                view.update(app, |this, cx| {
                                    if this.show_all_core_ranks != next {
                                        this.show_all_core_ranks = next;
                                        // A display lens that persists, like the tuner's collapse
                                        // flags: the mode is chosen once and expected back after
                                        // a restart.
                                        this.backend.update(cx, |b, _| {
                                            b.layout.analytics_cores_show_all = next;
                                            b.layout_dirty = true;
                                        });
                                        cx.notify();
                                    }
                                });
                            })
                            .render()
                            .into_any_element();
                        chart_card_ex(
                            t!("analytics.cores_title").to_string(),
                            subtitle,
                            Some(modes),
                            charts::core_totals_rank(
                                &data.core_days,
                                self.show_all_core_ranks,
                                p,
                                cx,
                            ),
                            p,
                            cx,
                        )
                    }),
            )
            .into_any_element()
    }

    /// Row of KPI tiles with deltas against the previous period.
    fn kpi_row(&self, d: &Summary, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let (cur, prev) = (&d.cur, &d.prev);
        // A missing or zero comparison value has no meaningful percentage
        // delta, so the KPI tile renders an em dash.
        let delta = |c: f64, pr: Option<f64>| -> Option<f64> {
            let pr = pr?;
            (pr.abs() > f64::EPSILON).then(|| (c - pr) / pr.abs() * 100.0)
        };
        let profit_el = colored_value(p, cur.profit, format!("{}", fmt_signed(cur.profit)));
        let dd_el = div()
            .text_color(moon(p.orange))
            .child(format!(
                "−{}{}",
                compact(cur.max_dd, 2),
                crate::analytics::pnl_suffix()
            ))
            .into_any_element();
        let avg_el = colored_value(p, cur.avg, fmt_signed(cur.avg));
        h_flex()
            .w_full()
            .gap(design::ui_px(cx, 8.0))
            .items_stretch()
            .child(kpi(
                p,
                cx,
                t!(
                    "analytics.kpi.profit",
                    unit = crate::analytics::pnl_unit_label()
                ),
                profit_el,
                delta(cur.profit, prev.as_ref().map(|v| v.profit)),
                false,
            ))
            .child(kpi(
                p,
                cx,
                t!("analytics.kpi.trades"),
                plain_value(p, cur.n.to_string()),
                delta(cur.n as f64, prev.as_ref().map(|v| v.n as f64)),
                false,
            ))
            .child(kpi(
                p,
                cx,
                t!("analytics.kpi.winrate"),
                plain_value(p, format!("{:.1}%", cur.winrate())),
                delta(cur.winrate(), prev.as_ref().map(|v| v.winrate())),
                false,
            ))
            .child(kpi(
                p,
                cx,
                t!("analytics.kpi.pf"),
                plain_value(p, format!("{:.2}", cur.pf)),
                delta(cur.pf, prev.as_ref().map(|v| v.pf)),
                false,
            ))
            .child(kpi(
                p,
                cx,
                t!("analytics.kpi.maxdd"),
                dd_el,
                delta(cur.max_dd, prev.as_ref().map(|v| v.max_dd)),
                true,
            ))
            .child(kpi(
                p,
                cx,
                t!("analytics.kpi.avg"),
                avg_el,
                delta(cur.avg, prev.as_ref().map(|v| v.avg)),
                false,
            ))
            .child(kpi(
                p,
                cx,
                t!("analytics.kpi.duration"),
                plain_value(
                    p,
                    format!("{:.0} {}", cur.avg_dur_min, t!("analytics.minutes")),
                ),
                delta(cur.avg_dur_min, prev.as_ref().map(|v| v.avg_dur_min)),
                true,
            ))
    }
}

fn plain_value(p: MoonPalette, text: String) -> AnyElement {
    div()
        .text_color(moon(p.text))
        .child(text)
        .into_any_element()
}

fn colored_value(p: MoonPalette, v: f64, text: String) -> AnyElement {
    div()
        .text_color(moon(sign_color(p, v)))
        .child(text)
        .into_any_element()
}

/// KPI tile: label (caption, muted) + large value + a ▲▼ delta.
/// `invert` — growth in this metric is bad (drawdown, duration).
fn kpi(
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
    label: impl std::fmt::Display,
    value: AnyElement,
    delta: Option<f64>,
    invert: bool,
) -> impl IntoElement {
    let delta_el = match delta {
        Some(d) if d.is_finite() && d.abs() > 0.05 => {
            let good = if invert { d < 0.0 } else { d > 0.0 };
            let col = if good { p.green } else { p.orange };
            h_flex()
                .gap(design::ui_px(cx, 4.0))
                .items_center()
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(col))
                        .child(format!(
                            "{} {:.1}%",
                            if d > 0.0 { "▲" } else { "▼" },
                            d.abs()
                        )),
                )
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(t!("analytics.vs_prev").to_string()),
                )
                .into_any_element()
        }
        _ => div()
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_muted))
            .child("—")
            .into_any_element(),
    };
    v_flex()
        .flex_1()
        .min_w(design::font_w_px(cx, 108.0))
        .gap(design::ui_px(cx, 3.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 9.0))
        .rounded(design::ui_px(cx, 8.0))
        .bg(moon(p.panel))
        .border_1()
        .border_color(moon(p.border))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_soft))
                .child(label.to_string()),
        )
        .child(
            div()
                .text_size(design::t_title(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .child(value),
        )
        .child(delta_el)
}

/// Chart card: title + subtitle + body.
fn chart_card(
    title: String,
    sub: String,
    body: AnyElement,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> impl IntoElement {
    chart_card_ex(title, sub, None, body, p, cx)
}

/// Chart card with an optional header control (a mode checkbox and the like).
fn chart_card_ex(
    title: String,
    sub: String,
    head_extra: Option<AnyElement>,
    body: AnyElement,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> impl IntoElement {
    let mut head = h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(design::t_title(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        );
    if let Some(extra) = head_extra {
        head = head.child(extra);
    }
    v_flex()
        .flex_1()
        .min_w_0()
        // In the elastic bottom the card fills the height it is given; in the
        // top rows (content-sized height) h_full degenerates into auto.
        .h_full()
        .gap(design::ui_px(cx, 2.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 10.0))
        .rounded(design::ui_px(cx, 8.0))
        .bg(moon(p.panel))
        .border_1()
        .border_color(moon(p.border))
        .child(head)
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .mb(design::ui_px(cx, 6.0))
                .child(sub),
        )
        .child(div().w_full().flex_1().min_h_0().child(body))
}

/// Render a card of ranked trades.
///
/// Args:
///     title: Localized card heading.
///     trades: Ranked trade rows.
///     zone: Zone the REPORT AXIS renders in. A top trade's `closedate` is a replicated value on
///         the core's own clock, so it must not travel through the user's display zone on top.
///     p: Active MoonUI palette.
///     cx: Analytics view context.
///
/// Returns:
///     Complete ranked-trades card.
fn top_card(
    title: String,
    trades: &[TopTrade],
    zone: chrono_tz::Tz,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> impl IntoElement {
    let mut list = v_flex().w_full().gap_0();
    // Header.
    list = list.child(
        h_flex()
            .w_full()
            .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
            .px(design::ui_px(cx, 8.0))
            .gap(design::ui_px(cx, 8.0))
            .items_center()
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .bg(moon(p.table_head))
            .child(
                div()
                    .w(design::font_w_px(cx, 96.0))
                    .child(t!("analytics.col.closed").to_string()),
            )
            .child(
                div()
                    .w(design::font_w_px(cx, 78.0))
                    .child(t!("analytics.col.coin").to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(t!("analytics.col.strategy").to_string()),
            )
            .child(div().child(t!("analytics.col.profit").to_string())),
    );
    for tr in trades {
        let profit_col = sign_color(p, tr.profit);
        list = list.child(
            h_flex()
                .w_full()
                .h(design::fit_h_px(cx, 25.0, 14.0, 5.5))
                .px(design::ui_px(cx, 8.0))
                .gap(design::ui_px(cx, 8.0))
                .items_center()
                .bg(moon(p.table_body))
                .border_t_1()
                .border_color(moon_alpha(p.border, 0.6))
                .child(
                    div()
                        .w(design::font_w_px(cx, 96.0))
                        .text_color(moon(p.text_soft))
                        .child(fmt_dm_hm(tr.closedate, zone)),
                )
                .child(
                    h_flex()
                        .w(design::font_w_px(cx, 78.0))
                        .gap(design::ui_px(cx, 4.0))
                        .items_center()
                        .child(div().min_w_0().truncate().child(tr.coin.clone()))
                        .child(
                            MoonBadge::new(if tr.is_short { "S" } else { "L" })
                                .tone(if tr.is_short {
                                    MoonTone::Negative
                                } else {
                                    MoonTone::Positive
                                })
                                .variant(MoonBadgeVariant::Soft)
                                .size(MoonBadgeSize::Tiny)
                                .render_with_palette(p),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(moon(p.text_soft))
                        .child(strat_display(&tr.strategy)),
                )
                .child(
                    div()
                        .text_color(moon(profit_col))
                        .child(fmt_signed(tr.profit)),
                ),
        );
    }
    v_flex()
        .flex_1()
        .min_w_0()
        .rounded(design::ui_px(cx, 8.0))
        .bg(moon(p.panel))
        .border_1()
        .border_color(moon(p.border))
        .overflow_hidden()
        .child(
            div()
                .px(design::ui_px(cx, 12.0))
                .py(design::ui_px(cx, 8.0))
                .text_size(design::t_title(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(list)
}

/// One stable row in the Insights card.
struct InsightRow {
    /// Short localized category that makes the five-row scan predictable.
    label: String,
    /// Primary strategy, coin, verdict, or hour.
    main: String,
    /// Compact right-pinned metric preserved when the middle column truncates.
    metric: String,
    /// Semantic colour for the metric.
    metric_color: u32,
    /// Complete localized conclusion shown when the compact row needs explanation.
    tooltip: Option<String>,
}

/// Build five insight slots without changing the existing eligibility rules or calculations.
///
/// Args:
///     d: Summary aggregates for the selected period.
///     p: Active Moon palette used to colour each metric by meaning.
///
/// Returns:
///     Strategy, contribution, risk, quality, and hour rows in stable order. Optional facts become
///     neutral placeholders so the card keeps the same scan pattern and height in every period.
fn insight_rows(d: &Summary, p: MoonPalette) -> [InsightRow; 5] {
    let missing = |label: String| InsightRow {
        label,
        main: "—".to_string(),
        metric: String::new(),
        metric_color: p.text_muted,
        tooltip: None,
    };

    let strategy = d
        .strategies
        .first()
        .filter(|group| group.profit > 0.0)
        .map(|best| {
            let name = strat_display(&best.name);
            let profit = fmt_signed_unit(best.profit);
            let wr = format!("{:.1}", best.winrate());
            InsightRow {
                label: t!("analytics.ins.label.strategy").to_string(),
                main: name.clone(),
                metric: t!("analytics.ins.metric.strategy", profit = profit, wr = wr).to_string(),
                metric_color: p.green,
                tooltip: Some(
                    t!(
                        "analytics.ins.best_strategy",
                        name = name,
                        profit = profit,
                        wr = wr
                    )
                    .to_string(),
                ),
            }
        })
        .unwrap_or_else(|| missing(t!("analytics.ins.label.strategy").to_string()));

    let contribution = (d.cur.profit > 0.0)
        .then(|| d.coins.first().filter(|group| group.profit > 0.0))
        .flatten()
        .and_then(|top| {
            let share = (top.profit / d.cur.profit * 100.0).round() as i64;
            (share > 10).then(|| InsightRow {
                label: t!("analytics.ins.label.contribution").to_string(),
                main: top.name.clone(),
                metric: t!("analytics.ins.metric.share", share = share).to_string(),
                metric_color: p.green,
                tooltip: Some(
                    t!("analytics.ins.top_coin", name = top.name, share = share).to_string(),
                ),
            })
        })
        .unwrap_or_else(|| missing(t!("analytics.ins.label.contribution").to_string()));

    let risk = d
        .coins
        .last()
        .filter(|group| group.profit < 0.0)
        .map(|worst| {
            let profit = fmt_signed_unit(worst.profit);
            InsightRow {
                label: t!("analytics.ins.label.risk").to_string(),
                main: worst.name.clone(),
                metric: profit.clone(),
                metric_color: p.orange,
                tooltip: Some(
                    t!(
                        "analytics.ins.worst_coin",
                        name = worst.name,
                        profit = profit
                    )
                    .to_string(),
                ),
            }
        })
        .unwrap_or_else(|| missing(t!("analytics.ins.label.risk").to_string()));

    let (pf_verdict, pf_color) = if d.cur.pf >= 2.0 {
        (t!("analytics.ins.pf_great").to_string(), p.green)
    } else if d.cur.pf >= 1.3 {
        (t!("analytics.ins.pf_good").to_string(), p.green)
    } else if d.cur.pf >= 1.0 {
        (t!("analytics.ins.pf_edge").to_string(), p.amber)
    } else {
        (t!("analytics.ins.pf_bad").to_string(), p.orange)
    };
    let quality = InsightRow {
        label: t!("analytics.ins.label.quality").to_string(),
        main: format!("{:.2} | {pf_verdict}", d.cur.pf),
        metric: t!(
            "analytics.ins.metric.streaks",
            w = d.cur.win_streak,
            l = d.cur.loss_streak
        )
        .to_string(),
        metric_color: pf_color,
        tooltip: Some(
            t!(
                "analytics.ins.pf",
                pf = format!("{:.2}", d.cur.pf),
                verdict = pf_verdict,
                w = d.cur.win_streak,
                l = d.cur.loss_streak
            )
            .to_string(),
        ),
    };

    let hour = d
        .best_hour
        .map(|(hour, profit, trades)| {
            let profit = fmt_signed_unit(profit);
            InsightRow {
                label: t!("analytics.ins.label.hour").to_string(),
                main: t!(
                    "analytics.ins.metric.hour_clock",
                    hour = format!("{hour:02}:00")
                )
                .to_string(),
                metric: t!("analytics.ins.metric.hour", profit = profit, n = trades).to_string(),
                metric_color: p.green,
                tooltip: Some(
                    t!(
                        "analytics.ins.best_hour",
                        hour = format!("{hour:02}:00"),
                        profit = profit,
                        n = trades
                    )
                    .to_string(),
                ),
            }
        })
        .unwrap_or_else(|| missing(t!("analytics.ins.label.hour").to_string()));

    [strategy, contribution, risk, quality, hour]
}

/// Render structured automatic conclusions for the selected period.
///
/// Args:
///     d: Summary aggregates for the selected period.
///     p: Active Moon palette.
///     cx: Analytics context used to resolve scaled row geometry.
///
/// Returns:
///     A five-row table that shares the neighbouring trade cards' header, row, and typography
///     rhythm while retaining each conclusion's full localized wording in a tooltip.
fn insights_card(d: &Summary, p: MoonPalette, cx: &Context<AnalyticsView>) -> impl IntoElement {
    let label_w = design::font_w_px(cx, 104.0);
    let metric_w = design::font_w_px(cx, 190.0);
    let row_h = design::fit_h_px(cx, 25.0, 14.0, 5.5);
    let mut list = v_flex().w_full().gap_0().child(
        h_flex()
            .w_full()
            .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
            .px(design::ui_px(cx, 8.0))
            .gap(design::ui_px(cx, 8.0))
            .items_center()
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .bg(moon(p.table_head))
            .child(
                div()
                    .flex_1()
                    .max_w(label_w)
                    .min_w_0()
                    .truncate()
                    .child(t!("analytics.ins.col.insight").to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(t!("analytics.ins.col.detail").to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .max_w(metric_w)
                    .min_w_0()
                    .truncate()
                    .text_right()
                    .child(t!("analytics.ins.col.result").to_string()),
            ),
    );
    for (ix, row) in insight_rows(d, p).into_iter().enumerate() {
        let mut element = h_flex()
            .id(("an-insight-row", ix))
            .w_full()
            .h(row_h)
            .min_w_0()
            .items_center()
            .gap(design::ui_px(cx, 8.0))
            .px(design::ui_px(cx, 8.0))
            .bg(moon(p.table_body))
            .border_t_1()
            .border_color(moon_alpha(p.border, 0.6))
            .child(
                div()
                    .flex_1()
                    .max_w(label_w)
                    .min_w_0()
                    .truncate()
                    .text_color(moon(p.text_soft))
                    .child(row.label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(moon(p.text_soft))
                    .child(row.main),
            )
            .child(
                div()
                    .flex_1()
                    .max_w(metric_w)
                    .min_w_0()
                    .truncate()
                    .text_right()
                    .text_color(moon(row.metric_color))
                    .child(row.metric),
            );
        if let Some(tooltip) = row.tooltip {
            element = element.tooltip(crate::panels::common::text_tooltip(tooltip));
        }
        list = list.child(element);
    }
    v_flex()
        .flex_1()
        .min_w_0()
        .rounded(design::ui_px(cx, 8.0))
        .bg(moon(p.panel))
        .border_1()
        .border_color(moon(p.border))
        .overflow_hidden()
        .child(
            div()
                .px(design::ui_px(cx, 12.0))
                .py(design::ui_px(cx, 8.0))
                .text_size(design::t_title(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .child(t!("analytics.insights").to_string()),
        )
        .child(list)
}

/// Format a top-table timestamp as `DD.MM.YY HH:MM` in the zone it is handed.
///
/// Args:
///     secs: Absolute UTC Unix seconds.
///     zone: Zone to render in — the report axis's for a replicated value, never the display
///         zone independently.
///
/// Returns:
///     Civil date-time label, or the shared formatter's fallback text.
pub(super) fn fmt_dm_hm(secs: i64, zone: chrono_tz::Tz) -> String {
    let s = moon_core::util::display_time::format_minute(secs, zone);
    // The shared formatter returns "YYYY-MM-DD HH:MM"; take "DD.MM.YY HH:MM".
    if s.len() >= 16 {
        format!("{}.{}.{} {}", &s[8..10], &s[5..7], &s[2..4], &s[11..16])
    } else {
        s
    }
}

/// Display name of a strategy: `strategyid = 0` means manual orders (no
/// strategy), so the bare "0" is replaced with a human-readable label.
pub(super) fn strat_display(name: &str) -> String {
    if name == "0" {
        t!("analytics.manual_orders").to_string()
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests;
