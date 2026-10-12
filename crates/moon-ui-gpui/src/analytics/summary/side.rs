//! Right column of the Summary tab: the "What matters" insights and the best/worst trades table.

use std::rc::Rc;

use gpui::*;
use moon_core::db::analytics::{Summary, TopTrade};
use moon_ui::{MoonPalette, MoonSegmentItem, MoonSegmentedControl, MoonTag, h_flex, v_flex};
use rust_i18n::t;

use super::model::{Insight, InsightTag, InsightTone};
use super::state::TradeSide;
use super::{StratLabel, fmt_dm_hm, fmt_signed, sign_color, strat_display_ex};
use crate::design;
use crate::design::{moon, moon_alpha};

/// Callbacks the trades card raises; the view owns what they change.
#[derive(Clone)]
pub(in crate::analytics) struct TradesActs {
    /// The best/worst control was clicked.
    pub(in crate::analytics) on_side: Rc<dyn Fn(TradeSide, &mut Window, &mut App)>,
}

/// Shared card frame of the side column.
fn card(selector: &'static str, cx: &App, p: MoonPalette) -> Div {
    v_flex()
        .debug_selector(move || selector.into())
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 8.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 10.0))
        .rounded(design::ui_px(cx, 8.0))
        .bg(moon(p.panel))
        .border_1()
        .border_color(moon(p.border))
}

/// Card title in the shared title face.
fn title(selector: &'static str, text: String, cx: &App) -> Div {
    div()
        .debug_selector(move || selector.into())
        .flex_none()
        .font_family(design::ui_font())
        .text_size(design::t_title(cx))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}

/// MoonTag face of one insight category: gains positive, losses warning, timing and quality info.
fn tag_el(tag: InsightTag) -> MoonTag {
    let (tag, key) = match tag {
        InsightTag::Leader => (MoonTag::positive(), "analytics.sum.ins.tag.leader"),
        InsightTag::Minus => (MoonTag::warning(), "analytics.sum.ins.tag.minus"),
        InsightTag::Day => (MoonTag::warning(), "analytics.sum.ins.tag.day"),
        InsightTag::Time => (MoonTag::info(), "analytics.sum.ins.tag.time"),
        InsightTag::Quality => (MoonTag::info(), "analytics.sum.ins.tag.quality"),
    };
    tag.mono(false).label(t!(key).to_string())
}

/// Palette token of an insight value.
fn tone_color(tone: InsightTone, p: MoonPalette) -> u32 {
    match tone {
        InsightTone::Gain => p.green,
        InsightTone::Loss => p.orange,
        InsightTone::Edge => p.orange,
    }
}

/// Caption under the insights title: "over N days" for the selected period on a daily grid,
/// "over the period" otherwise. N counts the period's civil days, not the days that traded.
///
/// Args:
///     d: Loaded Summary.
///
/// Returns:
///     The localized caption.
pub(in crate::analytics) fn period_caption(d: &Summary) -> String {
    match super::states::period_days((d.from, d.to)) {
        Some(n) if d.bucket_secs == 86_400 => {
            let n = n as usize;
            let key = crate::analytics::tuner::plural_key("analytics.sum.ins.for_days", n);
            t!(key, n = n).to_string()
        }
        _ => t!("analytics.sum.ins.for_period").to_string(),
    }
}

/// Render the "What matters" card.
///
/// Args:
///     items: Insights in their fixed order.
///     period_caption: Caption naming the period the insights cover.
///     p: Active palette.
///     cx: App context for scaled sizes.
///
/// Returns:
///     The card; an empty list says so instead of leaving a blank card.
pub(in crate::analytics) fn insights_el(
    items: &[Insight],
    period_caption: String,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let header = h_flex()
        .w_full()
        .flex_wrap()
        .items_baseline()
        .gap_x(design::ui_px(cx, 8.0))
        .child(title(
            "sum-ins-title",
            t!("analytics.sum.ins.title").to_string(),
            cx,
        ))
        .child(
            div()
                .min_w_0()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(period_caption),
        );
    let mut list = v_flex().w_full().gap(design::ui_px(cx, 8.0));
    if items.is_empty() {
        list = list.child(
            div()
                .debug_selector(|| "sum-ins-none".into())
                .text_size(design::t_body(cx))
                .text_color(moon(p.text_muted))
                .child(t!("analytics.sum.ins.none").to_string()),
        );
    }
    for (i, it) in items.iter().enumerate() {
        let row = h_flex()
            .debug_selector(move || format!("sum-ins-row-{i}"))
            .w_full()
            .items_start()
            .gap(design::ui_px(cx, 8.0))
            .child(
                div()
                    .debug_selector(move || format!("sum-ins-tag-{i}"))
                    .flex_none()
                    .child(tag_el(it.tag)),
            )
            // The fact wraps rather than truncates; the value beside it keeps its full width.
            .child(
                div()
                    .debug_selector(move || format!("sum-ins-what-{i}"))
                    .flex_1()
                    .min_w_0()
                    .text_size(design::t_body(cx))
                    .text_color(moon(p.text))
                    .child(it.what.clone()),
            )
            .child(
                div()
                    .debug_selector(move || format!("sum-ins-value-{i}"))
                    .flex_none()
                    .font_family(design::mono())
                    .text_size(design::t_body(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(moon(tone_color(it.value_tone, p)))
                    .child(it.value.clone()),
            );
        let mut entry = v_flex().w_full().gap(design::ui_px(cx, 2.0)).child(row);
        if let Some(why) = &it.why {
            entry = entry.child(
                div()
                    .debug_selector(move || format!("sum-ins-why-{i}"))
                    .w_full()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(why.clone()),
            );
        }
        list = list.child(entry);
    }
    card("sum-ins-card", cx, p)
        .child(header)
        .child(list)
        .into_any_element()
}

/// The trades the card lists for one side.
///
/// Args:
///     d: Loaded Summary.
///     side: Best or worst.
///
/// Returns:
///     The Summary's five best or five worst trades.
pub(in crate::analytics) fn side_rows(d: &Summary, side: TradeSide) -> &[TopTrade] {
    match side {
        TradeSide::Best => &d.best,
        TradeSide::Worst => &d.worst,
    }
}

/// The cells of one trades row, ready to render.
pub(in crate::analytics) struct TradeCells {
    /// Close time in the trade's display zone.
    pub(in crate::analytics) closed: String,
    /// Coin, never shortened.
    pub(in crate::analytics) coin: String,
    /// Trade direction for the side badge.
    pub(in crate::analytics) is_short: bool,
    /// Strategy cell: text, muted flag, full identity for the tooltip.
    pub(in crate::analytics) strategy: StratLabel,
    /// Signed profit text.
    pub(in crate::analytics) profit: String,
}

/// Turn one trade into its row cells.
pub(in crate::analytics) fn trade_cells(tr: &TopTrade, zone: chrono_tz::Tz) -> TradeCells {
    TradeCells {
        closed: fmt_dm_hm(tr.closedate, zone),
        coin: tr.coin.clone(),
        is_short: tr.is_short,
        strategy: strat_display_ex(
            &tr.strategy,
            &tr.strategy_id,
            tr.strategy_is_id,
            tr.strategy_alive,
        ),
        profit: fmt_signed(tr.profit),
    }
}

/// Tooltip of a strategy cell: the full label, then the full id when the label shortened one.
pub(in crate::analytics) fn strategy_tip(s: &StratLabel) -> String {
    match &s.full_id {
        Some(id) => format!(
            "{}
{id}",
            s.text
        ),
        None => s.text.clone(),
    }
}

/// Render the "Trades" card: best/worst switch and a table that scrolls sideways when narrow.
///
/// Args:
///     rows: Trades of the chosen side.
///     side: Which side is chosen.
///     zone: Zone the close times are printed in.
///     acts: Callbacks into the view.
///     p: Active palette.
///     cx: App context for scaled sizes and text measurement.
///
/// Returns:
///     The card; it schedules nothing.
pub(in crate::analytics) fn trades_el(
    rows: &[TopTrade],
    side: TradeSide,
    zone: chrono_tz::Tz,
    acts: &TradesActs,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let on_side = acts.on_side.clone();
    let seg = MoonSegmentedControl::new("an-sum-trades")
        .items([
            MoonSegmentItem::new("", t!("analytics.sum.trades.best5").to_string())
                .fit_width(cx, 40.0, 120.0)
                .selected(side == TradeSide::Best),
            MoonSegmentItem::new("", t!("analytics.sum.trades.worst5").to_string())
                .fit_width(cx, 40.0, 120.0)
                .selected(side == TradeSide::Worst),
        ])
        .on_click(move |ix, _, window, app| {
            let side = if ix == 0 {
                TradeSide::Best
            } else {
                TradeSide::Worst
            };
            on_side(side, window, app);
        });
    let header = h_flex()
        .w_full()
        .flex_wrap()
        .items_center()
        .gap_x(design::ui_px(cx, 10.0))
        .gap_y(design::ui_px(cx, 4.0))
        .child(title(
            "sum-trades-title",
            t!("analytics.sum.trades.title").to_string(),
            cx,
        ))
        .child(
            div()
                .debug_selector(|| "sum-trades-seg".into())
                .ml_auto()
                .flex_none()
                .child(seg),
        );

    let cells: Vec<TradeCells> = rows.iter().map(|tr| trade_cells(tr, zone)).collect();
    // One coin column for every row: the widest coin (mono, as the table inherits) plus the
    // badge, so the columns line up and no coin is ever cut.
    let badge_w =
        design::ui_caption_text_width(cx, "SHORT", 600.0) + f32::from(design::ui_px(cx, 14.0));
    let coin_w = cells
        .iter()
        .map(|c| design::mono_body_text_width(cx, &c.coin, 400.0))
        .fold(
            design::ui_body_text_width(cx, &t!("analytics.col.coin"), 400.0),
            f32::max,
        )
        + badge_w
        + f32::from(design::ui_px(cx, 4.0));
    // The close time never wraps: the column is the widest time as the row draws it (mono).
    let closed_w = px(cells
        .iter()
        .map(|c| design::mono_body_text_width(cx, &c.closed, 400.0))
        .fold(
            design::ui_caption_text_width(cx, &t!("analytics.col.closed"), 400.0),
            f32::max,
        )
        .ceil()
        + 1.0);
    let profit_w = design::font_w_px(cx, 64.0);
    let gap = design::ui_px(cx, 8.0);
    let row_base = |h: Pixels| {
        h_flex()
            .w_full()
            .h(h)
            .px(design::ui_px(cx, 6.0))
            .gap(gap)
            .items_center()
    };
    let mut table = v_flex()
        .debug_selector(|| "sum-trades-table".into())
        .w_full()
        .min_w(design::font_w_px(cx, 300.0) + px(coin_w))
        .text_size(design::t_body(cx))
        .child(
            row_base(design::fit_h_px(cx, 22.0, 12.0, 5.0))
                .font_family(design::ui_font())
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_soft))
                .bg(moon(p.table_head))
                .child(
                    div()
                        .w(closed_w)
                        .flex_none()
                        .child(t!("analytics.col.closed").to_string()),
                )
                .child(
                    div()
                        .w(px(coin_w))
                        .flex_none()
                        .child(t!("analytics.col.coin").to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(t!("analytics.col.strategy").to_string()),
                )
                .child(
                    div()
                        .min_w(profit_w)
                        .flex_none()
                        .text_right()
                        .child(t!("analytics.col.profit").to_string()),
                ),
        );
    for (ix, (c, tr)) in cells.into_iter().zip(rows).enumerate() {
        // The strategy cell is the direct child of the width-owning row: an intermediate flex
        // around truncating text collapses the whole line to an ellipsis.
        let tip = strategy_tip(&c.strategy);
        let strategy_cell = div()
            .id(("an-sum-trade-strat", ix))
            .debug_selector(move || format!("sum-trades-strat-{ix}"))
            .flex_1()
            .min_w_0()
            .truncate()
            .text_color(moon(if c.strategy.muted {
                p.text_muted
            } else {
                p.text_soft
            }))
            // Always: the cell truncates, so the tooltip is how a long name is read in full.
            .tooltip(crate::panels::common::text_tooltip_value(tip));
        table = table.child(
            row_base(design::fit_h_px(cx, 25.0, 14.0, 5.5))
                .debug_selector(move || format!("sum-trades-row-{ix}"))
                .border_t_1()
                .border_color(moon_alpha(p.border, 0.6))
                .child(
                    div()
                        .debug_selector(move || format!("sum-trades-closed-{ix}"))
                        .w(closed_w)
                        .flex_none()
                        .whitespace_nowrap()
                        .text_color(moon(p.text_soft))
                        .child(c.closed),
                )
                .child(
                    h_flex()
                        .debug_selector(move || format!("sum-trades-coin-{ix}"))
                        // At least the shared column, and never narrower than coin plus badge.
                        .min_w(px(coin_w))
                        .flex_none()
                        .gap(design::ui_px(cx, 4.0))
                        .items_center()
                        .child(
                            div()
                                .debug_selector(move || format!("sum-trades-coin-text-{ix}"))
                                .flex_none()
                                .child(c.coin),
                        )
                        .child(
                            div()
                                .debug_selector(move || format!("sum-trades-badge-{ix}"))
                                .flex_none()
                                .child(crate::panels::common::side_badge(c.is_short, p)),
                        ),
                )
                .child(strategy_cell.child(c.strategy.text))
                .child(
                    div()
                        .debug_selector(move || format!("sum-trades-profit-{ix}"))
                        .min_w(profit_w)
                        .flex_none()
                        .text_right()
                        .font_family(design::mono())
                        .text_color(moon(sign_color(p, tr.profit)))
                        .child(c.profit),
                ),
        );
    }
    card("sum-trades-card", cx, p)
        .child(header)
        .child(
            div()
                .id("an-sum-trades-scroll")
                .debug_selector(|| "sum-trades-scroll".into())
                .w_full()
                .overflow_x_scroll()
                .child(table),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests;
