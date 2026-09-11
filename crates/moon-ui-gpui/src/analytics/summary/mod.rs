//! "Summary" tab of the "Analytics" window: KPI cards compared against the
//! previous period, charts (cumulative profit + daily bars), the top 5
//! best/worst trades and automatic "insights". Layout follows the
//! analytics-mock artifact.

use gpui::*;
use moon_ui::{MoonPalette, MoonSegmentItem, MoonSegmentedControl, h_flex, v_flex};
use rust_i18n::t;

use super::AnalyticsView;
mod charts;
pub(super) use charts::PopupHover;
mod cumulative;
use crate::design;
use crate::design::{moon, moon_alpha};
use crate::load_state::Note;
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
    ///
    /// Args:
    ///     p: Active MoonUI palette.
    ///     chrome_width: Window's responsive width. The cumulative legend needs it to decide how
    ///         many core names fit before rendering because GPUI exposes no measured width then.
    ///     cx: Analytics view context.
    ///
    /// Returns:
    ///     Summary surface or the placeholder for its current load state.
    pub(super) fn summary_tab(
        &self,
        p: MoonPalette,
        chrome_width: f32,
        cx: &Context<Self>,
    ) -> AnyElement {
        let data = match self.data.view(|d| d.cur.n == 0) {
            Ok(d) => d.clone(),
            // An empty query result under a scope the viewing preset or the Auto rail narrowed
            // must still say so — otherwise every core hidden reads as "nothing happened",
            // rather than as the zero-core scope it actually is.
            Err(Note::Empty) => {
                let placeholder = super::note_el("an-summary-note", Note::Empty, 18.0, p, cx);
                let Some(marker) = self.summary_scope_marker().filter(|m| m.hides_anything())
                else {
                    return placeholder;
                };
                // `line`, not `facts`: nothing is drawn to the left of this caption, so the
                // footer tail's leading separator would open the sentence with a stray bullet.
                let text = marker.line();
                let tip = marker.tooltip(std::slice::from_ref(&text));
                return v_flex()
                    .child(
                        div()
                            .px(design::ui_px(cx, 18.0))
                            .pt(design::ui_px(cx, 18.0))
                            .child(
                                div()
                                    .id("an-summary-scope-marker")
                                    .text_size(design::t_caption(cx))
                                    .text_color(moon(p.text_muted))
                                    .tooltip(crate::panels::common::text_tooltip(tip))
                                    .child(text),
                            ),
                    )
                    .child(placeholder)
                    .into_any_element();
            }
            Err(note) => return super::note_el("an-summary-note", note, 18.0, p, cx),
        };
        // Core series colors come from the server's SETTINGS (ServerConfig.color, as in the core
        // selector), and stay there unless two of them would draw as one line — a whole exchange
        // given one colour is exactly what made the cumulative chart's twelve per-core curves
        // indistinguishable. ONE source for every consumer: the legend, the hover-popup dots and
        // the daily/kind bars all read this vector, so they cannot disagree.
        let core_colors: Vec<Hsla> = {
            let b = self.backend.read(cx);
            let configured: Vec<(u64, Option<[u8; 3]>)> = data
                .core_days
                .iter()
                .map(|c| {
                    (
                        c.uid,
                        b.config
                            .servers
                            .iter()
                            .find(|s| s.id == c.uid)
                            .map(|s| s.color),
                    )
                })
                .collect();
            charts::distinct_core_colors(&configured, p)
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
                        // Scope marker: whether the active Auto preset OR the Classic viewing
                        // preset hid a core from this window's read, distinct from the line-cap
                        // fact above, which only names how many of the (already scoped) cores
                        // got a curve.
                        let marker = self.summary_scope_marker();
                        // `line`, not `facts`: this caption is its OWN flex child with a gap
                        // beside it, never a tail spliced onto a head, so a leading separator
                        // would introduce nothing.
                        let marker_line = marker.as_ref().map_or_else(String::new, |m| m.line());
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
                            .children((!marker_line.is_empty()).then(|| {
                                let text = marker_line.clone();
                                // Built from the SAME string the caption renders, per decision 1.
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
                            v_flex()
                                .w_full()
                                .gap(design::ui_px(cx, 4.0))
                                .children(cumulative::core_legend(
                                    &data.core_days,
                                    &core_colors,
                                    // The two chart cards split the row evenly inside the tab's
                                    // own padding; an ESTIMATE, exactly as `period_bar` measures
                                    // its own budget, because no measured width exists yet.
                                    ((chrome_width
                                        - design::ui_value(cx, 10.0) * 2.0
                                        - design::ui_value(cx, 8.0))
                                        / 2.0
                                        - design::ui_value(cx, 12.0) * 2.0)
                                        .max(0.0),
                                    p,
                                    cx,
                                ))
                                .child(cumulative::cumulative_area(
                                    &data.days,
                                    &data.core_days,
                                    &core_colors,
                                    self.hover_cum_bucket,
                                    data.bucket_secs,
                                    self.bound_zone(),
                                    p,
                                    cx,
                                ))
                                .into_any_element(),
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
                        "an-top-best",
                        t!("analytics.best_trades").to_string(),
                        &data.best,
                        self.bound_zone(),
                        p,
                        cx,
                    ))
                    .child(top_card(
                        "an-top-worst",
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
                pct_delta(cur.profit, prev.as_ref().map(|v| v.profit)),
                DeltaGood::Up,
            ))
            .child(kpi(
                p,
                cx,
                t!("analytics.kpi.trades"),
                plain_value(p, cur.n.to_string()),
                pct_delta(cur.n as f64, prev.as_ref().map(|v| v.n as f64)),
                DeltaGood::Up,
            ))
            .child(kpi(
                p,
                cx,
                t!("analytics.kpi.winrate"),
                plain_value(p, format!("{:.1}%", cur.winrate())),
                pct_delta(cur.winrate(), prev.as_ref().map(|v| v.winrate())),
                DeltaGood::Up,
            ))
            .child(kpi(
                p,
                cx,
                t!("analytics.kpi.pf"),
                plain_value(p, format!("{:.2}", cur.pf)),
                pct_delta(cur.pf, prev.as_ref().map(|v| v.pf)),
                DeltaGood::Up,
            ))
            .child(kpi(
                p,
                cx,
                t!("analytics.kpi.maxdd"),
                dd_el,
                pct_delta(cur.max_dd, prev.as_ref().map(|v| v.max_dd)),
                DeltaGood::Down,
            ))
            .child(kpi(
                p,
                cx,
                t!("analytics.kpi.avg"),
                avg_el,
                pct_delta(cur.avg, prev.as_ref().map(|v| v.avg)),
                DeltaGood::Up,
            ))
            .child(kpi(
                p,
                cx,
                t!("analytics.kpi.duration"),
                plain_value(
                    p,
                    format!("{:.0} {}", cur.avg_dur_min, t!("analytics.minutes")),
                ),
                pct_delta(cur.avg_dur_min, prev.as_ref().map(|v| v.avg_dur_min)),
                DeltaGood::Neither,
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

/// Which direction of change is GOOD for a metric, and therefore which colour its delta takes.
///
/// Two of the seven tiles are not "up is good", and each is a different case. Max Drawdown is
/// stored as a MAGNITUDE — `Summary::max_dd` is `max(peak - cum)` and never negative — so ▲ means
/// a DEEPER drawdown and is the bad direction, even though the value printed beside it carries a
/// leading minus. Average duration has no good direction at all: a longer hold is neither a win
/// nor a loss, so its delta is toned down rather than claiming one.
///
/// That neutral tone is `text_soft`, not `text_muted`, and the difference is not cosmetic:
/// `text_muted` on `panel` measures 3.50:1 on the Terminal palette and 3.70:1 on the Light one,
/// under the 4.5:1 floor for normal text at caption size. It is the right token for the em dash
/// and for the "vs previous period" suffix — chrome, and an ABSENCE of a figure — but the neutral
/// delta is a FIGURE the user reads. `text_soft` clears the floor in both themes (5.09:1 and
/// 6.96:1) and is already this tile's own label colour, so nothing new enters the palette.
enum DeltaGood {
    /// Growth is good: profit, trades, winrate, profit factor, average trade.
    Up,
    /// Shrinkage is good: drawdown.
    Down,
    /// Neither direction is good: duration.
    Neither,
}

impl DeltaGood {
    /// Pick the palette token for a delta whose ROUNDED sign is `sign`.
    ///
    /// Args:
    ///     p: Active MoonUI palette.
    ///     sign: Sign of the delta, classified from the rounded percentage.
    ///
    /// Returns:
    ///     `0xRRGGBB` token for the delta text.
    fn tone(self, p: MoonPalette, sign: fmt::DeltaSign) -> u32 {
        match self {
            Self::Up => sign.pick(p.green, p.orange, p.text_soft),
            Self::Down => sign.pick(p.orange, p.green, p.text_soft),
            Self::Neither => p.text_soft,
        }
    }
}

/// Percentage change of `cur` against the previous period's `prev`.
///
/// A missing or zero comparison value has no meaningful percentage delta, so the KPI tile renders
/// an em dash. The denominator is `|prev|`, so the sign of the result is the sign of the CHANGE and
/// never of the baseline.
///
/// Args:
///     cur: Current-period value.
///     prev: Previous-period value, when one was loaded.
///
/// Returns:
///     Change in percent, or `None` when there is nothing to compare against.
fn pct_delta(cur: f64, prev: Option<f64>) -> Option<f64> {
    let prev = prev?;
    (prev.abs() > f64::EPSILON).then(|| (cur - prev) / prev.abs() * 100.0)
}

/// Arrow text and palette token for one KPI delta — the whole colour rule of the row, in one place.
///
/// [`fmt::pct`] classifies the sign from the ROUNDED percentage, so the arrow and the colour can
/// never disagree with the digits rendered beside them, and a delta that rounds away to zero — or
/// was never comparable — returns `None` and leaves the tile its em dash. `pct` prints that sign
/// into the text too, which the arrow already carries, hence the trim.
///
/// Routing the digits through `pct` also puts this row on the module's ONE rounding rule —
/// [`fmt::round_to`]'s half-away-from-zero, the rule [`fmt::signed_amount`] documents at length —
/// instead of `{:.1}`'s half-to-even. Every exactly representable midpoint therefore moves by a
/// tenth of a point: `0.25%` renders `0.3%` where it used to render `0.2%`, and a delta of exactly
/// `0.05%` now shows `0.1%` where it used to fall under the old `> 0.05` guard into the em dash.
/// That is the point of the change, not a side effect of it — the alternative is a KPI row that
/// rounds differently from every other figure on the window.
///
/// Args:
///     delta: Percentage change against the previous period, when comparable.
///     good: Which direction of change this metric calls good.
///     p: Active MoonUI palette.
///
/// Returns:
///     The `"▲ 168.7%"`-shaped text with its `0xRRGGBB` token, or `None` for the em dash.
fn delta_parts(delta: Option<f64>, good: DeltaGood, p: MoonPalette) -> Option<(String, u32)> {
    let (text, sign) = delta.and_then(|d| fmt::pct(d, 1))?;
    if sign == fmt::DeltaSign::Zero {
        return None;
    }
    Some((
        format!(
            "{} {}",
            sign.pick("▲", "▼", ""),
            text.trim_start_matches('-')
        ),
        good.tone(p, sign),
    ))
}

/// KPI tile: label (caption, muted) + large value + a ▲▼ delta.
/// `good` — which direction of change this metric calls good; see [`DeltaGood`].
fn kpi(
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
    label: impl std::fmt::Display,
    value: AnyElement,
    delta: Option<f64>,
    good: DeltaGood,
) -> impl IntoElement {
    let delta_el = match delta_parts(delta, good, p) {
        Some((text, col)) => h_flex()
            .gap(design::ui_px(cx, 4.0))
            .items_center()
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(col))
                    .child(text),
            )
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.vs_prev").to_string()),
            )
            .into_any_element(),
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
                .font_family(design::ui_font())
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
                .font_family(design::ui_font())
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
                .font_family(design::ui_font())
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
///     key: Which of the two ranked-trade cards this is. It only DISCRIMINATES: the element
///         id the strategy cell actually takes is named inside this function, beside the cell.
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
    key: &'static str,
    title: String,
    trades: &[TopTrade],
    zone: chrono_tz::Tz,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> impl IntoElement {
    // The strategy cell's identity is named HERE, where the cell is built, and carries the
    // card discriminator so the two cards cannot collide on one row index.
    let cell_id: SharedString = format!("an-top-strat-{key}").into();
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
    for (ix, tr) in trades.iter().enumerate() {
        let profit_col = sign_color(p, tr.profit);
        let strategy = strat_display_ex(
            &tr.strategy,
            &tr.strategy_id,
            tr.strategy_is_id,
            tr.strategy_alive,
        );
        // The cell is the direct child of the width-owning row below: an intermediate flex
        // around truncating text collapses the whole line to an ellipsis.
        let mut strategy_cell = div()
            .id((cell_id.clone(), ix))
            .flex_1()
            .min_w_0()
            .truncate()
            .text_color(moon(if strategy.muted {
                p.text_muted
            } else {
                p.text_soft
            }));
        if let Some(id) = strategy.full_id {
            strategy_cell = strategy_cell.tooltip(crate::panels::common::text_tooltip(id));
        }
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
                        .child(crate::panels::common::side_badge(tr.is_short, p)),
                )
                .child(strategy_cell.child(strategy.text))
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
                .font_family(design::ui_font())
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
            // The group key is `strategyid@core_uid`; the id half is the identity a nameless
            // strategy is labelled by.
            let id = best.key.rsplit_once('@').map_or("", |(id, _)| id);
            let label = strat_display_ex(&best.name, id, best.name_is_id, best.alive);
            let profit = fmt_signed_unit(best.profit);
            let wr = format!("{:.1}", best.winrate());
            // The row is one line and already truncates, so the FULL id rides in the sentence
            // the row's own tooltip prints — the compact cell keeps the shortened form.
            let spelled = label
                .full_id
                .as_ref()
                .map_or_else(|| label.text.clone(), |id| format!("{} ({id})", label.text));
            InsightRow {
                label: t!("analytics.ins.label.strategy").to_string(),
                main: label.text,
                metric: t!("analytics.ins.metric.strategy", profit = profit, wr = wr).to_string(),
                metric_color: p.green,
                tooltip: Some(
                    t!(
                        "analytics.ins.best_strategy",
                        name = spelled,
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
                .font_family(design::ui_font())
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
///
/// The short form for callers that hold no identity to fall back on — the Tuner's three. A
/// caller that DOES know the row's strategy id takes [`strat_display_ex`] instead, which is the
/// only way a row with no resolved name gets a label rather than a raw 19-digit hash.
pub(super) fn strat_display(name: &str) -> String {
    // The "0" rule stays HERE, not in `strat_display_ex`. This form's callers hand it text that
    // may be an id, so "0" means the manual-orders identity; `strat_display_ex`'s callers say
    // whether their text is an id, and there a strategy a user NAMED "0" must keep that name.
    if name == "0" {
        return t!("analytics.manual_orders").to_string();
    }
    name.to_string()
}

/// A strategy cell: what to print, whether it is the app speaking rather than the user, and the
/// full identity behind a shortened one.
struct StratLabel {
    /// Text rendered in the Summary cell.
    pub text: String,
    /// The label is OURS, not a name the user gave — render it muted so the two never look alike.
    pub muted: bool,
    /// Present only when [`Self::text`] shortened an identity, so a tooltip can show all of it.
    pub full_id: Option<String>,
}

/// Turn one strategy identity into the cell the Summary renders.
///
/// The unresolved case is REPORTED by the database layer (`GroupStat::name_is_id`,
/// `TopTrade::strategy_is_id`), never guessed from the text: a strategy a user named "12345" is
/// indistinguishable from an id fallback by comparison, and guessing would relabel it.
///
/// Args:
///     name: Display text as the database layer produced it.
///     id: The row's own `strategyid` text, used only when `is_id` says `name` IS that text.
///     is_id: Whether `name` is the id fallback rather than a name.
///     alive: Head status with `GroupStat::alive`'s encoding: `None` when status is unavailable,
///         `0` when the database lacks the pair, and `1` or `2` when it has the pair. Only `0`
///         produces the deleted-strategy label.
///
/// Returns:
///     The cell's text, whether it is muted, and the full identity when the text shortened one.
fn strat_display_ex(name: &str, id: &str, is_id: bool, alive: Option<i64>) -> StratLabel {
    // A RESOLVED name is printed as it is, before any rule keyed on text: a strategy a user
    // named "0" is not the manual-orders identity, and relabelling it would be exactly the
    // guessing this signature exists to remove.
    if !is_id {
        return StratLabel {
            text: name.to_string(),
            muted: false,
            full_id: None,
        };
    }
    // `strategyid = 0` is manual orders — a real identity with its own label that no strategy
    // database will ever name, so it reaches here as an unresolved id and stops here.
    if name == "0" {
        return StratLabel {
            text: t!("analytics.manual_orders").to_string(),
            muted: false,
            full_id: None,
        };
    }
    // `alive == Some(0)` is the database saying it does not have this pair — deleted. Anything
    // else (a live head whose name is blank, or no strategy database at all) is NOT a deletion,
    // and saying so would be a claim nobody checked.
    let key = if alive == Some(0) {
        "analytics.strategy_deleted"
    } else {
        "analytics.strategy_unnamed"
    };
    StratLabel {
        text: t!(key, id = short_id(id)).to_string(),
        muted: true,
        full_id: Some(id.to_string()),
    }
}

/// Tail of a strategy id, for a label that must fit a table cell.
///
/// Moonbot writes `strategyid` as a 19-digit signed hash, which is both unreadable and wider
/// than the column. The tail is what a human compares against the tooltip's full id, and it is
/// DECIMAL because every other place this app prints a strategy id is (`analytics.purge.*`).
///
/// Args:
///     id: Full id text as the report row stored it.
///
/// Returns:
///     The id itself when it is already short, else an ellipsis and its last six characters.
fn short_id(id: &str) -> String {
    let n = id.chars().count();
    if n <= 8 {
        return id.to_string();
    }
    format!("…{}", id.chars().skip(n - 6).collect::<String>())
}

#[cfg(test)]
mod tests;
