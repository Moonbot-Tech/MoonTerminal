//! Charts of the "Summary" tab: daily bars (divs, green/orange by sign) and the
//! per-core totals (canvas quads), plus the bucket-popup chrome both these and the
//! cumulative chart share. The cumulative chart itself lives in `cumulative`.

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
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
pub(super) fn bucket_label(secs: i64, bucket: i64) -> String {
    if bucket < 86_400 {
        let s = moon_core::db::fmt_unix(secs);
        // "YYYY-MM-DD HH:MM" → "HH:MM"
        if s.len() >= 16 {
            s[11..16].to_string()
        } else {
            s
        }
    } else {
        dm(secs)
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

/// "dd.mm" from unix seconds (axis labels).
pub(super) fn dm(secs: i64) -> String {
    let s = moon_core::db::fmt_unix(secs);
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
pub(super) fn daily_bars(
    days: &[DayPoint],
    cores: &[CoreSeries],
    colors: &[Hsla],
    hover: Option<usize>,
    bucket: i64,
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
        // Bars are equal flex cells: bucket `bi` is the (bi+0.5)/n-th of the width.
        .map(|bi| {
            let frac = (bi as f32 + 0.5) / n as f32;
            bucket_popup(days, cores, colors, bi, frac, bucket, PopupMode::Day, p, cx)
        });
    let first = days.first().map(|d| d.start).unwrap_or(0);
    let last = days.last().map(|d| d.start).unwrap_or(0);
    div()
        .relative()
        .w_full()
        .child(v_flex().w_full().gap(px(4.0)).child(row).child(axis_row(
            p,
            bucket_label(first, bucket),
            bucket_label(last, bucket),
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
pub(super) fn bucket_popup(
    days: &[DayPoint],
    cores: &[CoreSeries],
    colors: &[Hsla],
    bi: usize,
    frac: f32,
    bucket: i64,
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
        .map(|d| bucket_label(d.start, bucket))
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
