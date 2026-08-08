//! Charts of the "Summary" tab: daily bars, horizontal per-core rankings, and the
//! bucket-popup chrome these charts share with the cumulative chart. The cumulative
//! chart itself lives in `cumulative`.

use std::ops::Range;

use gpui::*;
use moon_ui::{
    MoonPalette, MoonProgress, MoonScrollbarVisibility, MoonVirtualList, h_flex, v_flex,
};
use rust_i18n::t;

use super::super::AnalyticsView;
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::{CoreSeries, DayPoint, KindStat};

pub(super) const CHART_H: f32 = 170.0;

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

/// Label of one bucket: the HOUR when the grid is finer than a day (a single-day period),
/// the date otherwise. Without this every hourly bucket would be titled with the same date.
///
/// Args:
///     secs: Bucket start as UTC Unix seconds.
///     bucket: Civil bucket width in seconds.
///     zone: Selected IANA display zone.
///
/// Returns:
///     `HH:MM` for hourly grids or `DD.MM` for wider grids.
pub(super) fn bucket_label(secs: i64, bucket: i64, zone: chrono_tz::Tz) -> String {
    if bucket < 86_400 {
        let s = moon_core::util::display_time::format_minute(secs, zone);
        // "YYYY-MM-DD HH:MM" → "HH:MM"
        if s.len() >= 16 {
            s[11..16].to_string()
        } else {
            s
        }
    } else {
        dm(secs, zone)
    }
}

/// A core's colour looked up by uid — the per-type popup knows uids, not series indices.
pub(super) fn core_color_by_uid(
    cores: &[CoreSeries],
    colors: &[Hsla],
    uid: u64,
    p: MoonPalette,
) -> Hsla {
    match cores.iter().position(|c| c.uid == uid) {
        Some(ci) => core_color(colors, ci, p),
        None => moon(fallback_core_color(p, uid as usize)),
    }
}

/// Format UTC Unix seconds as selected-zone `DD.MM` for axis labels.
///
/// Args:
///     secs: Absolute UTC Unix seconds.
///     zone: Selected IANA display zone.
///
/// Returns:
///     Civil date label, or the shared formatter's fallback text.
pub(super) fn dm(secs: i64, zone: chrono_tz::Tz) -> String {
    let s = moon_core::util::display_time::format_minute(secs, zone);
    if s.len() >= 10 {
        format!("{}.{}", &s[8..10], &s[5..7])
    } else {
        s
    }
}

/// Daily profit bars: green upward / orange downward from the zero line, with
/// the value labelled above a green bar / below a red one (while the bars are
/// few). Hovering a column pops up that day's per-core breakdown through the shared
/// `bucket_popup` — the same card the cumulative chart opens, in `Day` mode.
///
/// Args:
///     days: Ordered analytical buckets.
///     cores: Per-core series aligned to `days`.
///     colors: Resolved per-core theme colors.
///     hover: Hovered bucket index, if any.
///     bucket: Civil bucket width in seconds.
///     zone: Selected IANA display zone.
///     p: Active MoonUI palette.
///     cx: Analytics view context.
///
/// Returns:
///     Complete daily/hourly bar chart.
pub(super) fn daily_bars(
    days: &[DayPoint],
    cores: &[CoreSeries],
    colors: &[Hsla],
    hover: Option<usize>,
    bucket: i64,
    zone: chrono_tz::Tz,
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
                    .child(div().w_full().flex().justify_center().child(format!(
                        "{}{}",
                        moon_core::util::fmt::compact(d.profit, 0),
                        crate::analytics::pnl_suffix()
                    ))),
            );
        }
        row = row.child(col);
    }
    let popup = hover
        .filter(|bi| *bi < n && !cores.is_empty())
        // Bars are equal flex cells: bucket `bi` is the (bi+0.5)/n-th of the width.
        .map(|bi| {
            let frac = (bi as f32 + 0.5) / n as f32;
            bucket_popup(
                days,
                cores,
                colors,
                bi,
                frac,
                bucket,
                zone,
                PopupMode::Day,
                p,
                cx,
            )
        });
    let first = days.first().map(|d| d.start).unwrap_or(0);
    let last = days.last().map(|d| d.start).unwrap_or(0);
    div()
        .relative()
        .w_full()
        .child(v_flex().w_full().gap(px(4.0)).child(row).child(axis_row(
            p,
            bucket_label(first, bucket, zone),
            bucket_label(last, bucket, zone),
        )))
        .children(popup)
        .into_any_element()
}

/// Rows a bucket popup lists before it would run off the screen.
const POPUP_ROWS: usize = 16;

/// A core's colour: the server's own, or the cycled fallback when it has no config entry.
pub(super) fn core_color(colors: &[Hsla], ci: usize, p: MoonPalette) -> Hsla {
    colors
        .get(ci)
        .copied()
        .unwrap_or_else(|| moon(fallback_core_color(p, ci)))
}

/// One line of a popup: a coloured dot, a label, and `+500 (123)`.
pub(super) struct PopupRow {
    pub label: String,
    pub dot: Hsla,
    pub value: f64,
    pub trades: i64,
}

/// THE popup of the Summary tab. Every chart builds its own rows and hands them here, so
/// the card, the row cap, the "…N more" tail and the anchoring exist once: the per-bucket
/// core split, the running totals and the per-type core split are all this one card.
///
/// `frac` is where the hovered column sits across the chart's width — the CALLER's
/// business, since the bars are equal flex cells (`bi/n`) while the curve puts its points
/// at `bi/(n-1)`, and a shared guess would anchor the card away from what it describes.
pub(super) fn popup_card(
    title: String,
    total: f64,
    trades: i64,
    mut rows: Vec<PopupRow>,
    frac: f32,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    // The CAP keeps the biggest by ABSOLUTE value: sorting by signed value first and
    // truncating would drop the worst losers, which are the rows worth reading.
    let found = rows.len();
    if found > POPUP_ROWS {
        rows.sort_by(|a, b| b.value.abs().total_cmp(&a.value.abs()));
        rows.truncate(POPUP_ROWS);
    }
    // Then DISPLAY by value: earners on top, losers at the bottom.
    rows.sort_by(|a, b| b.value.total_cmp(&a.value));
    let hidden = found.saturating_sub(rows.len());
    let mut card = popup_shell(p, cx).child(popup_head(title, total, trades, p, cx));
    for r in rows {
        card = card.child(popup_core_row(r.label, r.dot, r.value, r.trades, p, cx));
    }
    // The cap is never silent: without this the rows visibly fail to add up to the header.
    if hidden > 0 {
        card = card.child(
            div()
                .text_color(moon(p.text_muted))
                .child(t!("analytics.popup_more", n = hidden).to_string()),
        );
    }
    anchor_popup(card, frac, cx)
}

/// Which numbers a bucket popup reports — the ONE thing the two time charts' popups differ in.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PopupMode {
    /// Just that bucket: what the daily-bars chart draws.
    Day,
    /// Everything up to and including it: what the cumulative curve draws.
    Running,
}

/// Empty card of a bucket popup — shared chrome, so the cumulative chart's popup and the
/// daily one cannot drift apart in looks.
fn popup_shell(p: MoonPalette, cx: &Context<AnalyticsView>) -> Div {
    v_flex()
        .gap(px(2.0))
        .px(design::ui_px(cx, 8.0))
        .py(design::ui_px(cx, 6.0))
        .rounded(design::ui_px(cx, 6.0))
        .bg(moon(p.panel_high))
        .border_1()
        .border_color(moon(p.border))
        .shadow_md()
        .text_size(design::t_caption(cx))
}

/// `+500 (123)` — profit and the trade count behind it, the one shape every popup number
/// uses so a value can never be mistaken for a count.
fn profit_trades(v: f64, trades: i64) -> String {
    format!("{} ({trades})", super::fmt_signed(v))
}

/// Popup header: the bucket's date on the left, `Σ <total> (<trades>)` on the right. The
/// total lives in the HEADER because with dozens of cores the list's bottom is off screen.
fn popup_head(
    date: String,
    total: f64,
    trades: i64,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> impl IntoElement {
    h_flex()
        .justify_between()
        .gap(design::ui_px(cx, 10.0))
        .pb(px(1.0))
        .border_b_1()
        .border_color(moon_alpha(p.border, 0.6))
        .child(div().text_color(moon(p.text)).child(date))
        .child(
            h_flex()
                .gap(design::ui_px(cx, 4.0))
                .child(div().text_color(moon(p.text_muted)).child("Σ"))
                .child(
                    div()
                        .text_color(moon(super::sign_color(p, total)))
                        .child(profit_trades(total, trades)),
                ),
        )
}

/// One core's line in a bucket popup: colour dot, name, `+500 (123)`.
fn popup_core_row(
    name: String,
    dot: Hsla,
    v: f64,
    trades: i64,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> impl IntoElement {
    h_flex()
        .gap(design::ui_px(cx, 5.0))
        .items_center()
        .child(
            div()
                .flex_none()
                .w(design::ui_px(cx, 6.0))
                .h(design::ui_px(cx, 6.0))
                .rounded_full()
                .bg(dot),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(moon(p.text_soft))
                .child(name),
        )
        .child(
            div()
                .flex_none()
                .text_color(moon(super::sign_color(p, v)))
                .child(profit_trades(v, trades)),
        )
}

/// Anchor a popup to bucket `frac` of the chart's width, inside its relative container: in
/// the right third it opens to the LEFT of the column, otherwise to the right. `deferred`
/// paints it ON TOP of everything — without it the card hid under the cards drawn later.
fn anchor_popup(card: Div, frac: f32, cx: &Context<AnalyticsView>) -> AnyElement {
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

/// Popup of bucket `bi`: the date, `Σ` of the whole bucket, and every core that traded by
/// then as `name +500 (123)`, biggest profit first.
///
/// `mode` is the only difference between the two time charts: `Day` reports the bucket
/// alone (what the bars draw), `Running` everything up to and including it (what the
/// cumulative curve draws). Rendering goes through the shared [`popup_card`].
///
/// Args:
///     days: Ordered authoritative bucket totals.
///     cores: Per-core series aligned to `days`.
///     colors: Resolved per-core theme colors.
///     bi: Selected bucket index.
///     frac: Horizontal bucket position from zero through one.
///     bucket: Civil bucket width in seconds.
///     zone: Selected IANA display zone.
///     mode: Per-bucket or running-total interpretation.
///     p: Active MoonUI palette.
///     cx: Analytics view context.
///
/// Returns:
///     Deferred popup anchored beside the selected bucket.
pub(super) fn bucket_popup(
    days: &[DayPoint],
    cores: &[CoreSeries],
    colors: &[Hsla],
    bi: usize,
    frac: f32,
    bucket: i64,
    zone: chrono_tz::Tz,
    mode: PopupMode,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    // A core is listed when it TRADED by this bucket, not when its value is non-zero:
    // in `Running` mode a core whose total happens to cross zero right here would vanish.
    let items: Vec<(usize, f64, i64)> = cores
        .iter()
        .enumerate()
        .filter_map(|(ci, c)| {
            let (v, t) = match mode {
                PopupMode::Day => (
                    c.per_bucket.get(bi).copied().unwrap_or(0.0),
                    c.per_bucket_trades.get(bi).copied().unwrap_or(0),
                ),
                PopupMode::Running => (
                    c.per_bucket.iter().take(bi + 1).sum(),
                    c.per_bucket_trades.iter().take(bi + 1).sum(),
                ),
            };
            (t > 0).then_some((ci, v, t))
        })
        .collect();
    // Header totals come from `days`, the authoritative series — not from summing the rows,
    // which the row cap would truncate.
    let (total, trades) = match mode {
        PopupMode::Day => days
            .get(bi)
            .map(|d| (d.profit, d.trades))
            .unwrap_or((0.0, 0)),
        PopupMode::Running => days
            .iter()
            .take(bi + 1)
            .fold((0.0, 0), |(s, t), d| (s + d.profit, t + d.trades)),
    };
    let rows: Vec<PopupRow> = items
        .into_iter()
        .map(|(ci, value, trades)| PopupRow {
            label: cores[ci].name.clone(),
            dot: core_color(colors, ci, p),
            value,
            trades,
        })
        .collect();
    let title = days
        .get(bi)
        .map(|d| bucket_label(d.start, bucket, zone))
        .unwrap_or_default();
    popup_card(title, total, trades, rows, frac, p, cx)
}

/// Profit per STRATEGY TYPE: one bar per type, green up / orange down, the type's name and
/// number under it. Hovering a bar opens the cores behind that type through the shared
/// popup. This replaces the daily bars on a single-day period, where "per day" is one
/// column and says nothing.
pub(super) fn kind_bars(
    kinds: &[KindStat],
    cores: &[CoreSeries],
    colors: &[Hsla],
    hover: Option<usize>,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    if kinds.is_empty() {
        return div().h(px(CHART_H)).into_any_element();
    }
    // A non-finite profit would turn the whole scale into NaN and feed px(NaN) into layout.
    let fin = |v: f64| if v.is_finite() { v } else { 0.0 };
    let vmax = kinds
        .iter()
        .map(|k| fin(k.profit))
        .fold(0.0f64, f64::max)
        .max(1e-6);
    let vmin = kinds
        .iter()
        .map(|k| fin(k.profit))
        .fold(0.0f64, f64::min)
        .min(0.0);
    let span = (vmax - vmin).max(1e-6);
    let up_frac = (vmax / span) as f32;
    // Value labels stay readable only while the bars are few — the same rule the daily bars
    // use. They are `whitespace_nowrap`, so past this they smear over each other.
    let labels_on = kinds.len() <= 10;
    let pad_top = if labels_on { 13.0f32 } else { 0.0 };
    let pad_bottom = if labels_on && vmin < 0.0 {
        13.0f32
    } else {
        0.0
    };
    let area_h = (CHART_H - pad_top - pad_bottom).max(10.0);
    let zero_from_bottom = pad_bottom + area_h * (1.0 - up_frac);
    let n = kinds.len();

    let mut row = h_flex()
        .w_full()
        .h(px(CHART_H))
        .items_end()
        .gap(design::ui_px(cx, 4.0));
    for (ki, k) in kinds.iter().enumerate() {
        let frac = (k.profit.abs() / span) as f32;
        let bar_h = (frac * area_h).max(if k.trades > 0 { 1.5 } else { 0.0 });
        let bottom = if k.profit >= 0.0 {
            zero_from_bottom
        } else {
            (zero_from_bottom - bar_h).max(0.0)
        };
        let mut col = div()
            .id(SharedString::from(format!("an-kb-{ki}")))
            .flex_1()
            .min_w_0()
            .relative()
            .h_full()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    if this.hover_kind != Some(ki) {
                        this.hover_kind = Some(ki);
                        cx.notify();
                    }
                } else if this.hover_kind == Some(ki) {
                    this.hover_kind = None;
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
                    .bg(moon(if k.profit >= 0.0 { p.green } else { p.orange })),
            );
        if labels_on {
            // Value above a green bar / below a red one — pad_top/pad_bottom reserve the room.
            col = col.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(if k.profit >= 0.0 {
                        bottom + bar_h + 2.0
                    } else {
                        (bottom - 12.0).max(0.0)
                    }))
                    .text_size(design::t_caption(cx))
                    .whitespace_nowrap()
                    .text_color(moon(super::sign_color(p, k.profit)))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .justify_center()
                            .child(profit_trades(k.profit, k.trades)),
                    ),
            );
        }
        if hover == Some(ki) {
            col = col.bg(moon_alpha(p.text_muted, 0.07));
        }
        row = row.child(col);
    }
    // Type names under the bars, on the same flex grid.
    let mut labels = h_flex().w_full().flex_none().gap(design::ui_px(cx, 4.0));
    for k in kinds {
        labels = labels.child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_center()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_soft))
                .child(kind_label(&k.kind)),
        );
    }
    let popup = hover
        .and_then(|ki| kinds.get(ki).map(|k| (ki, k)))
        .map(|(ki, k)| {
            let rows: Vec<PopupRow> = k
                .cores
                .iter()
                .map(|c| PopupRow {
                    label: c.name.clone(),
                    dot: core_color_by_uid(cores, colors, c.uid, p),
                    value: c.profit,
                    trades: c.trades,
                })
                .collect();
            // Bars are equal flex cells: bar `ki` is the (ki+0.5)/n-th of the width.
            let frac = (ki as f32 + 0.5) / n as f32;
            popup_card(kind_label(&k.kind), k.profit, k.trades, rows, frac, p, cx)
        });
    div()
        .relative()
        .w_full()
        .child(v_flex().w_full().gap(px(4.0)).child(row).child(labels))
        .children(popup)
        .into_any_element()
}

/// A strategy type's display name; an unknown type (no strategies DB) shows a dash rather
/// than an empty column nobody can identify.
fn kind_label(kind: &str) -> String {
    if kind.trim().is_empty() {
        "—".to_string()
    } else {
        kind.to_string()
    }
}

/// Maximum rows in either half of the default per-core overview.
const CORE_OVERVIEW_LIMIT: usize = 10;

/// Display snapshot for one core-ranking row owned by a virtual-list factory.
#[derive(Clone)]
struct CoreRankRow {
    /// Durable identity used to keep element IDs stable across repaints.
    uid: u64,
    /// User-facing server name.
    name: SharedString,
    /// Exact period total in the active Analytics profit unit.
    total: f64,
    /// Magnitude relative to the largest absolute core result, in percent.
    magnitude_pct: f32,
}

/// Compact facts shown under the per-core card title.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CoreRankStats {
    /// Number of cores with trades in the selected period.
    pub total: usize,
    /// Number whose unrounded total is above zero.
    pub profitable: usize,
    /// Number whose unrounded total is below zero.
    pub losing: usize,
    /// Best core divided by a positive net result; losses may make this exceed 100 percent.
    pub leader_share_pct: Option<f64>,
}

/// Split a descending list into non-overlapping leader and outsider ranges.
///
/// Args:
///     len: Number of ranked cores.
///     limit: Maximum rows allocated to either side.
///
/// Returns:
///     The leading range and the trailing range. When fewer than `2 * limit` rows exist, the
///     outsider range starts after the leaders instead of repeating a core in both columns.
fn overview_ranges(len: usize, limit: usize) -> (Range<usize>, Range<usize>) {
    let leaders_end = len.min(limit);
    let outsiders_start = len.saturating_sub(limit).max(leaders_end);
    (0..leaders_end, outsiders_start..len)
}

/// Summarize signs and concentration for the per-core ranking header.
///
/// Args:
///     cores: Profit-descending core series for the selected period.
///
/// Returns:
///     Counts by unrounded sign plus the leading core's share of a positive net result. The share
///     is absent for zero or losing periods because that ratio has no useful interpretation.
pub(super) fn core_rank_stats(cores: &[CoreSeries]) -> CoreRankStats {
    let net: f64 = cores.iter().map(|core| core.total).sum();
    let leader = cores.iter().map(|core| core.total).fold(0.0f64, f64::max);
    CoreRankStats {
        total: cores.len(),
        profitable: cores.iter().filter(|core| core.total > 0.0).count(),
        losing: cores.iter().filter(|core| core.total < 0.0).count(),
        leader_share_pct: (net > f64::EPSILON && leader > 0.0).then_some(leader / net * 100.0),
    }
}

/// Convert database series into owned, normalized rows for a `'static` virtual-list factory.
///
/// Args:
///     cores: Profit-descending core series for the selected period.
///
/// Returns:
///     Owned rows whose largest absolute result has a 100-percent bar.
fn core_rank_rows(cores: &[CoreSeries]) -> Vec<CoreRankRow> {
    let scale = cores
        .iter()
        .map(|core| core.total.abs())
        .fold(0.0f64, f64::max)
        .max(f64::EPSILON);
    cores
        .iter()
        .map(|core| CoreRankRow {
            uid: core.uid,
            name: core.name.clone().into(),
            total: core.total,
            magnitude_pct: (core.total.abs() / scale * 100.0) as f32,
        })
        .collect()
}

/// Render one horizontal core-ranking row with a MoonUI progress bar.
///
/// Args:
///     id_prefix: Stable namespace separating overview and all-mode element IDs.
///     row: Owned display snapshot for the core.
///     p: Active Moon palette.
///     name_w: Maximum width reserved for a readable server name.
///     value_w: Width reserved for the signed total.
///     text_size: Active body text size.
///
/// Returns:
///     One fixed-height row suitable for `MoonVirtualList`.
fn core_rank_row(
    id_prefix: &'static str,
    row: CoreRankRow,
    p: MoonPalette,
    name_w: Pixels,
    value_w: Pixels,
    text_size: Pixels,
) -> AnyElement {
    h_flex()
        .size_full()
        .min_w_0()
        .items_center()
        .gap(px(8.0))
        .px(px(6.0))
        .child(
            div()
                .flex_1()
                .max_w(name_w)
                .min_w_0()
                .truncate()
                .text_size(text_size)
                .text_color(moon(p.text_soft))
                .child(row.name),
        )
        .child(
            div().flex_1().min_w_0().child(
                MoonProgress::new(format!("{id_prefix}-bar-{}", row.uid))
                    .value(row.magnitude_pct)
                    .color(super::sign_color(p, row.total))
                    .height(7.0)
                    .render(),
            ),
        )
        .child(
            div()
                .flex_none()
                .w(value_w)
                .text_right()
                .whitespace_nowrap()
                .text_size(text_size)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(moon(super::sign_color(p, row.total)))
                .child(super::fmt_signed(row.total)),
        )
        .into_any_element()
}

/// Render the default two-column leaders/outsiders overview in a virtual list.
///
/// Args:
///     rows: Owned profit-descending core rows.
///     p: Active Moon palette.
///     cx: Analytics context used to resolve scaled row geometry.
///
/// Returns:
///     A two-column ranking whose shared vertical scrollbar remains usable at the card's minimum
///     height and whose two sides never repeat the same core.
fn core_rank_overview(
    rows: Vec<CoreRankRow>,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    let (leaders, outsiders) = overview_ranges(rows.len(), CORE_OVERVIEW_LIMIT);
    let leaders = rows[leaders].to_vec();
    let outsiders: Vec<_> = rows[outsiders].iter().rev().cloned().collect();
    let count = leaders.len().max(outsiders.len());
    let row_h = f32::from(design::fit_h_px(cx, 28.0, 14.0, 7.0));
    let name_w = design::font_w_px(cx, 180.0);
    let value_w = design::font_w_px(cx, 82.0);
    let text_size = design::t_body(cx);
    let gap = design::ui_px(cx, 10.0);
    let scrollbar_gutter = design::ui_px(cx, 8.0);
    let list = MoonVirtualList::new("an-core-rank-overview", count, row_h, move |ix, _, _| {
        let left = leaders
            .get(ix)
            .cloned()
            .map(|row| core_rank_row("an-core-leader", row, p, name_w, value_w, text_size))
            .unwrap_or_else(|| div().into_any_element());
        let right = outsiders
            .get(ix)
            .cloned()
            .map(|row| core_rank_row("an-core-outsider", row, p, name_w, value_w, text_size))
            .unwrap_or_else(|| div().into_any_element());
        h_flex()
            .size_full()
            .min_w_0()
            .gap(gap)
            .pr(scrollbar_gutter)
            .child(div().flex_1().min_w_0().child(left))
            .child(div().flex_1().min_w_0().child(right))
    })
    .surface(false)
    .border(false)
    .radius(0.0)
    .scrollbar_visibility(MoonScrollbarVisibility::Always);
    v_flex()
        .size_full()
        .min_h_0()
        .gap(design::ui_px(cx, 2.0))
        .child(
            h_flex()
                .w_full()
                .gap(gap)
                .text_size(design::t_caption(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(moon(p.text_muted))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px(design::ui_px(cx, 6.0))
                        .child(t!("analytics.cores_leaders").to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px(design::ui_px(cx, 6.0))
                        .child(t!("analytics.cores_outsiders").to_string()),
                ),
        )
        .child(div().flex_1().min_h_0().child(list))
        .into_any_element()
}

/// Render every core in one profit-descending virtual list.
///
/// Args:
///     rows: Owned profit-descending core rows.
///     p: Active Moon palette.
///     cx: Analytics context used to resolve scaled row geometry.
///
/// Returns:
///     The complete ranking with an always-visible MoonUI scrollbar.
fn core_rank_all(
    rows: Vec<CoreRankRow>,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    let count = rows.len();
    let row_h = f32::from(design::fit_h_px(cx, 28.0, 14.0, 7.0));
    let name_w = design::font_w_px(cx, 280.0);
    let value_w = design::font_w_px(cx, 92.0);
    let text_size = design::t_body(cx);
    let scrollbar_gutter = design::ui_px(cx, 8.0);
    MoonVirtualList::new("an-core-rank-all", count, row_h, move |ix, _, _| {
        div().size_full().pr(scrollbar_gutter).child(
            rows.get(ix)
                .cloned()
                .map(|row| core_rank_row("an-core-all", row, p, name_w, value_w, text_size))
                .unwrap_or_else(|| div().into_any_element()),
        )
    })
    .surface(false)
    .border(false)
    .radius(0.0)
    .scrollbar_visibility(MoonScrollbarVisibility::Always)
    .into_any_element()
}

/// Render period totals per core as a readable horizontal ranking.
///
/// Args:
///     cores: Profit-descending core series for the selected period.
///     show_all: Whether to show the complete one-column ranking instead of the two-column
///         leaders/outsiders overview.
///     p: Active Moon palette.
///     cx: Analytics context used to resolve scaled geometry.
///
/// Returns:
///     A virtualized ranking that fills the elastic bottom card.
pub(super) fn core_totals_rank(
    cores: &[CoreSeries],
    show_all: bool,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    if cores.is_empty() {
        return div().flex_1().into_any_element();
    }
    let rows = core_rank_rows(cores);
    if show_all {
        core_rank_all(rows, p, cx)
    } else {
        core_rank_overview(rows, p, cx)
    }
}

#[cfg(test)]
mod tests;

/// X-axis labels of the daily-bars chart: the first and last date.
fn axis_row(p: MoonPalette, left: String, right: String) -> AnyElement {
    h_flex()
        .w_full()
        .justify_between()
        .child(muted_caption(p, left))
        .child(muted_caption(p, right))
        .into_any_element()
}

pub(super) fn muted_caption(p: MoonPalette, text: String) -> Div {
    div().text_color(moon(p.text_muted)).child(text)
}
