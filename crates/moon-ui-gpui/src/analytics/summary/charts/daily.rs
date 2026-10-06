//! Summary chart daily helpers.

use super::super::derived::DailyLabels;
use super::*;

/// Daily profit bars: green upward / orange downward from the zero line, with
/// the value labelled above a green bar / below a red one (while the bars are
/// few). Hovering a column pops up that day's per-core breakdown through the shared
/// `bucket_popup` — the same card the cumulative chart opens, in `Day` mode.
///
/// Args:
///     days: Ordered analytical buckets.
///     cores: Per-core series aligned to `days`.
///     colors: Resolved per-core theme colors.
///     daily: Cached candidate text and measured-width-keyed thinning for this snapshot.
///     hover: Hovered bucket index, if any.
///     bucket: Civil bucket width in seconds.
///     zone: Selected IANA display zone.
///     p: Active MoonUI palette.
///     cx: Analytics view context.
///
/// Returns:
///     Complete daily/hourly bar chart.
pub(in crate::analytics::summary) fn daily_bars(
    days: &[DayPoint],
    cores: &[CoreSeries],
    colors: &[Hsla],
    daily: &DailyLabels,
    hover: Option<usize>,
    bucket: i64,
    zone: chrono_tz::Tz,
    p: MoonPalette,
    cx: &Context<AnalyticsView>,
) -> AnyElement {
    let chart_h = design::ui_px(cx, CHART_H);
    if days.is_empty() {
        return div().h(chart_h).into_any_element();
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
    // Which bars keep a label: as many as fit side by side at the caption size, biggest
    // days first. Never a font shrink — the label is at `t_caption` or it is not drawn.
    // Candidates and their widest-by-character choice are formatted once per snapshot/unit.
    let texts = &daily.texts;
    // The label WIDTH is text and is measured at the caption size, so it already follows the
    // Font slider; the plot width is layout and must not (see `PLOT_W_NOMINAL`). Scaling both
    // together would let the room grow with the very setting that makes the labels wider.
    let label_w = daily.label_w(|text| design::mono_caption_text_width(cx, text, LABEL_WEIGHT));
    let (labelled, _) = daily.ensure_labelled(label_w);
    // Space reserved for the labels: always on top (above the tallest green
    // bar), on the bottom only when there is a negative value (the label goes
    // BELOW a red bar). Bars scale into the remaining height, so the numbers
    // are never covered by a column. Derived from the caption size rather than a
    // fixed 13px, which was sized for the 8px text this chart used to draw and
    // stopped clearing the line box the moment the Font slider moved.
    // Text-derived height plus a small fixed clearance, and the two take DIFFERENT scales on
    // purpose: `t_caption` follows the Font slider, the clearance goes through `ui_px` like
    // every other piece of chrome (`cumulative.rs` sizes its own band the same way).
    let label_band = f32::from(design::t_caption(cx)) * 1.4 + f32::from(design::ui_px(cx, 2.0));
    // The band is unconditional now: `thinned_labels` always keeps the last bucket and that one
    // is drawn even on a quiet day, so there is no longer a period that reserves room for
    // labels it never draws.
    let pad_top = label_band;
    let pad_bottom = if vmin < 0.0 { label_band } else { 0.0 };
    let area_h = (f32::from(chart_h) - pad_top - pad_bottom).max(10.0);
    let zero_from_bottom = pad_bottom + area_h * (1.0 - up_frac);
    let n = days.len();

    let mut row = h_flex()
        .w_full()
        .h(chart_h)
        .items_end()
        .gap(px(if n > 120 { 0.0 } else { 1.0 }));
    for (bi, d) in days.iter().enumerate() {
        let frac = (d.profit.abs() / span) as f32;
        let bar_h = (frac * area_h).max(if d.trades > 0 { 1.5 } else { 0.0 });
        // Positives grow up from the zero line, negatives grow down.
        let bottom = if d.profit >= 0.0 {
            zero_from_bottom
        } else {
            (zero_from_bottom - bar_h).max(pad_bottom - label_band)
        };
        let mut col = div()
            .id(ElementId::NamedInteger("an-db".into(), bi as u64))
            .flex_1()
            .relative()
            .h_full()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                chart_hover(this, PopupKey::Daily(bi), *hovered, false, cx);
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
        // A quiet bucket normally draws no number — an empty day has nothing to report. The LAST
        // bucket is the exception and is drawn whatever it holds, zero included: the right edge
        // of the chart is where a reader looks for where the period ended, and a missing number
        // there reads as missing DATA rather than as a flat day.
        let is_last = bi + 1 == n;
        if labelled.contains(&bi) && (d.trades > 0 || is_last) {
            // Label: above a green bar / below a red one (the space is
            // reserved by pad_top/pad_bottom, so no bar covers the number).
            let label_bottom = if d.profit >= 0.0 {
                bottom + bar_h + 2.0
            } else {
                (bottom - label_band).max(0.0)
            };
            // The label is wider than its column and does not wrap — otherwise
            // "333" got clipped to "33". The overhang is half a label on each
            // side, so it matches the width the thinning pass kept the columns
            // apart by; neighbours can no longer touch.
            let over = px(label_w / 2.0);
            // Except at the right edge, where that same overhang would push half the number past
            // the plot and into the card's padding. The last label spends its whole overhang on
            // the LEFT and ends flush with its own column, which is the plot's edge;
            // `thinned_labels` separates its neighbours against that shifted centre.
            let band = if is_last {
                div().left(px(-label_w)).right_0()
            } else {
                div().left(-over).right(-over)
            };
            let text = div().w_full().flex().child(texts[bi].clone());
            let text = if is_last {
                text.justify_end()
            } else {
                text.justify_center()
            };
            col = col.child(
                band.absolute()
                    .bottom(px(label_bottom))
                    .text_size(design::t_caption(cx))
                    .whitespace_nowrap()
                    .text_color(moon(super::sign_color(p, d.profit)))
                    .child(text),
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
