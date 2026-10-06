//! Summary chart ranking helpers.

use super::*;

/// Maximum rows in either half of the default per-core overview.
const CORE_OVERVIEW_LIMIT: usize = 10;

/// Display snapshot for one core-ranking row owned by a virtual-list factory.
#[derive(Clone)]
pub(super) struct CoreRankRow {
    /// Durable identity used to keep element IDs stable across repaints.
    pub(super) uid: u64,
    /// User-facing server name.
    pub(super) name: SharedString,
    /// Exact period total in the active Analytics profit unit.
    pub(super) total: f64,
    /// Magnitude relative to the largest absolute core result, in percent.
    pub(super) magnitude_pct: f32,
}

/// Compact facts shown under the per-core card title.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::analytics::summary) struct CoreRankStats {
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
pub(super) fn overview_ranges(len: usize, limit: usize) -> (Range<usize>, Range<usize>) {
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
pub(in crate::analytics::summary) fn core_rank_stats(cores: &[CoreSeries]) -> super::CoreRankStats {
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
pub(super) fn core_rank_rows(cores: &[CoreSeries]) -> Vec<CoreRankRow> {
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
pub(super) fn core_rank_row(
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
pub(super) fn core_rank_overview(
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
pub(super) fn core_rank_all(
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
pub(in crate::analytics::summary) fn core_totals_rank(
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
