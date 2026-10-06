//! Summary chart bucket helpers.

use super::*;

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
pub(in crate::analytics::summary) fn bucket_popup(
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
    let key = match mode {
        PopupMode::Day => PopupKey::Daily(bi),
        PopupMode::Running => PopupKey::Cumulative(bi),
    };
    popup_card(title, total, trades, rows, frac, key, p, cx)
}

/// Profit per STRATEGY TYPE: one bar per type, green up / orange down, the type's name and
/// number under it. Hovering a bar opens the cores behind that type through the shared
/// popup. This replaces the daily bars on a single-day period, where "per day" is one
/// column and says nothing.
pub(in crate::analytics::summary) fn kind_bars(
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
                chart_hover(this, PopupKey::Kind(ki), *hovered, false, cx);
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
            popup_card(
                kind_label(&k.kind),
                k.profit,
                k.trades,
                rows,
                frac,
                PopupKey::Kind(ki),
                p,
                cx,
            )
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
pub(super) fn kind_label(kind: &str) -> String {
    if kind.trim().is_empty() {
        "—".to_string()
    } else {
        kind.to_string()
    }
}
