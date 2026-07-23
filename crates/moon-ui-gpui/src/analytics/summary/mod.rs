//! "Summary" tab of the "Analytics" window: KPI cards compared against the
//! previous period, charts (cumulative profit + daily bars), the top 5
//! best/worst trades and automatic "insights". Layout follows the
//! analytics-mock artifact.

use gpui::*;
use moon_ui::{MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonPalette, MoonTone, h_flex, v_flex};
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
/// (the report `Profit` column), nothing in USDT mode. Every profit figure on the window goes
/// through here, so the unit follows the toolbar switch everywhere at once. The em dash for a
/// non-finite value is never given a unit.
pub(super) fn fmt_signed(v: f64) -> String {
    let s = fmt_signed_plain(v);
    if s == "—" {
        s
    } else {
        format!("{}{}", s, super::pnl_suffix())
    }
}

/// Signed profit carrying the FULL unit for prose (the insights sentences): "+15.34%" in percent
/// mode, "+15.34 USDT" in money mode. [`fmt_signed`] leaves a money figure unitless because its
/// column header or KPI label supplies the "USDT" — a free-standing sentence has no such label, so
/// the word rides along here. A non-finite value stays a bare em dash.
fn fmt_signed_unit(v: f64) -> String {
    let s = fmt_signed(v);
    if s == "—" || super::pnl_is_pct() {
        s
    } else {
        format!("{s} USDT")
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
                                t!("analytics.cum_sub", unit = crate::analytics::pnl_unit_label())
                                    .to_string()
                            },
                            Some(head),
                            cumulative::cumulative_area(
                                &data.days,
                                &data.core_days,
                                &core_colors,
                                self.hover_cum_bucket,
                                data.bucket_secs,
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
                                t!("analytics.kinds_sub", unit = crate::analytics::pnl_unit_label())
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
                                t!("analytics.daily_sub", unit = crate::analytics::pnl_unit_label())
                                    .to_string(),
                                charts::daily_bars(
                                    &data.days,
                                    &data.core_days,
                                    &core_colors,
                                    self.hover_daily_bucket,
                                    data.bucket_secs,
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
                    .items_start()
                    .child(top_card(
                        t!("analytics.best_trades").to_string(),
                        &data.best,
                        p,
                        cx,
                    ))
                    .child(top_card(
                        t!("analytics.worst_trades").to_string(),
                        &data.worst,
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
            // The bottom chart is ELASTIC: pinned to the bottom of the window
            // and stretching up to the nearest card (flex_1); the bars are
            // painted on canvas across the whole available height, with a
            // minimum so it cannot collapse.
            .child(
                div()
                    .flex_1()
                    .min_h(design::ui_px(cx, 170.0))
                    .w_full()
                    .px(design::ui_px(cx, 10.0))
                    .pb(design::ui_px(cx, 10.0))
                    .child(chart_card(
                        t!("analytics.cores_title").to_string(),
                        t!("analytics.cores_sub").to_string(),
                        charts::core_totals_bars(&data.core_days, &core_colors, p, cx),
                        p,
                        cx,
                    )),
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
                t!("analytics.kpi.profit", unit = crate::analytics::pnl_unit_label()),
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
fn top_card(
    title: String,
    trades: &[TopTrade],
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
                        .child(fmt_dm_hm(tr.closedate)),
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

/// "Insights": automatic conclusions drawn from the period's aggregates.
fn insights_card(d: &Summary, p: MoonPalette, cx: &Context<AnalyticsView>) -> impl IntoElement {
    let mut lines: Vec<String> = Vec::new();
    if let Some(best) = d.strategies.first().filter(|g| g.profit > 0.0) {
        lines.push(
            t!(
                "analytics.ins.best_strategy",
                name = strat_display(&best.name),
                profit = fmt_signed_unit(best.profit),
                wr = format!("{:.1}", best.winrate())
            )
            .to_string(),
        );
    }
    if d.cur.profit > 0.0 {
        if let Some(top) = d.coins.first().filter(|g| g.profit > 0.0) {
            let share = (top.profit / d.cur.profit * 100.0).round() as i64;
            if share > 10 {
                lines
                    .push(t!("analytics.ins.top_coin", name = top.name, share = share).to_string());
            }
        }
    }
    if let Some(worst) = d.coins.last().filter(|g| g.profit < 0.0) {
        lines.push(
            t!(
                "analytics.ins.worst_coin",
                name = worst.name,
                profit = fmt_signed_unit(worst.profit)
            )
            .to_string(),
        );
    }
    let pf_verdict = if d.cur.pf >= 2.0 {
        t!("analytics.ins.pf_great")
    } else if d.cur.pf >= 1.3 {
        t!("analytics.ins.pf_good")
    } else if d.cur.pf >= 1.0 {
        t!("analytics.ins.pf_edge")
    } else {
        t!("analytics.ins.pf_bad")
    };
    lines.push(
        t!(
            "analytics.ins.pf",
            pf = format!("{:.2}", d.cur.pf),
            verdict = pf_verdict,
            w = d.cur.win_streak,
            l = d.cur.loss_streak
        )
        .to_string(),
    );
    if let Some((h, profit, n)) = d.best_hour {
        lines.push(
            t!(
                "analytics.ins.best_hour",
                hour = format!("{h:02}:00"),
                profit = fmt_signed_unit(profit),
                n = n
            )
            .to_string(),
        );
    }

    let mut list = v_flex().w_full().gap(design::ui_px(cx, 7.0));
    for line in lines {
        list = list.child(
            h_flex()
                .items_start()
                .gap(design::ui_px(cx, 8.0))
                .child(
                    div()
                        .flex_none()
                        .mt(design::ui_px(cx, 4.0))
                        .w(design::ui_px(cx, 5.0))
                        .h(design::ui_px(cx, 5.0))
                        .rounded(px(1.0))
                        .bg(moon(p.accent)),
                )
                .child(div().flex_1().min_w_0().child(line)),
        );
    }
    v_flex()
        .flex_1()
        .min_w_0()
        .gap(design::ui_px(cx, 2.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 10.0))
        .rounded(design::ui_px(cx, 8.0))
        .bg(moon(p.panel))
        .border_1()
        .border_color(moon(p.border))
        .child(
            div()
                .text_size(design::t_title(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .child(t!("analytics.insights").to_string()),
        )
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .mb(design::ui_px(cx, 6.0))
                .child(t!("analytics.insights_sub").to_string()),
        )
        .child(list)
}

/// "dd.mm.yy hh:mm" UTC for the top tables — the full date: over monthly
/// periods a bare time with no day says nothing.
pub(super) fn fmt_dm_hm(secs: i64) -> String {
    let s = moon_core::db::fmt_unix(secs);
    // fmt_unix → "YYYY-MM-DD HH:MM"; we take "DD.MM.YY HH:MM".
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
