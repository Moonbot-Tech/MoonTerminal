//! The hover card over a closed-trade arrow: what the chart deliberately does not draw.
//!
//! The chart spends colour on the trade's SIDE and shape on its ACTION, which leaves no channel for
//! profit and loss — see [`moon_chart::trade_marks`]. That figure lives here instead, where there is
//! room to give it a currency and a sign.
//!
//! Split of responsibilities, mirroring [`super::news`]:
//! - the ARROWS are uploaded through the own-pass adapter (`chartdx::trade_history_sync`), because
//!   the chart scrolls with the live edge every frame without notifying GPUI;
//! - the CARD is a GPUI overlay, so wrapping, theme colours and the font-size slider come for free.
//!
//! # Why the hit test reads a retained snapshot
//!
//! It hit-tests the clusters the pane last UPLOADED, never a fresh clustering of the live view. The
//! rebuild signature quantizes the view scale into buckets, so inside one bucket the view keeps
//! moving while the GPU buffers do not; re-clustering from the live scale would answer about a
//! picture that is not on screen, and the card would list trades belonging to an arrow the user
//! cannot see.
//!
//! # Why the card is not gated behind Ctrl
//!
//! News marks sit in one fixed row along the axis, so a modifier is discoverable there. A trade
//! arrow is a few pixels wide and scattered across the plot; a Ctrl-gated card on it would simply
//! never be found. The cost is a GPUI repaint when the hovered arrow changes, bounded by the same
//! movement threshold the order-line hover already pays and by the hit test failing for almost
//! every pointer position.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, MoonSurface, MoonSurfaceVariant, h_flex, v_flex};
use rust_i18n::t;

use moon_chart::trade_marks::TradeMarkAt;
use moon_core::db::ChartTradeRecord;
use moon_core::figures::Proj;
use moon_core::util::fmt;
use moon_core::util::now_unix_ms_i64;

use super::ChartPanel;
use crate::design;

/// Card width in logical pixels, before the UI-scale slider. Narrower than the news card: a trade
/// row is figures rather than prose, so it needs no room for a wrapped headline.
const CARD_W: f32 = 268.0;
/// Gap between the cursor and the card's nearest edge, on both axes.
const CARD_GAP: f32 = 12.0;
/// Gap kept between the card and the slot edge it is pushed against.
const CARD_EDGE_INSET: f32 = 6.0;
/// Floor for the card's height, below which clipping it further shows nothing useful.
const CARD_MIN_H: f32 = 44.0;
/// How many trades the card spells out before collapsing the rest into a counter. A dense cluster
/// can hold hundreds; the card exists to identify what is under the cursor, not to be a report.
const CARD_MAX_ITEMS: usize = 6;
/// Decimals used for the percentage figure, which is the same scale the Report's percent column
/// shows.
const PCT_DECIMALS: usize = 2;
/// Side labels. NOT dictionary entries: `locales/README.md` lists LONG and SHORT among the terms
/// this product deliberately leaves untranslated, beside BUY, SELL and PANIC SELL. They are the
/// instrument's own vocabulary and must stay consistent with the untranslated chart actions.
const SIDE_LONG: &str = "LONG";
/// See [`SIDE_LONG`].
const SIDE_SHORT: &str = "SHORT";

/// This chart's trade-arrow hover.
#[derive(Default)]
pub(super) struct TradeHoverState {
    /// The arrow under the cursor and the trades it stands for.
    hover: Option<TradeHover>,
    /// Point of the most recent hit test, carrying the same movement threshold the order-line and
    /// news hovers use.
    probe: Option<(f32, f32)>,
}

/// One resolved hover: which arrow grew, and which trades the card must list.
///
/// The record indices are resolved AT HIT TIME rather than at render time. They have to be: a
/// cluster's members index the pane's own filtered mark list, and the map back to the panel's
/// records lives beside the clusters in the retained geometry. Resolving later would mean reading a
/// snapshot that may already have been rebuilt under a different filter.
#[derive(Clone, PartialEq, Eq, Debug)]
struct TradeHover {
    /// Pane the arrow belongs to. Every pane draws only its own core's trades, so the mark index
    /// below is meaningless without it.
    pane: usize,
    /// The hovered ACTION as `(mark index in this pane's filtered numbering, buy)`.
    ///
    /// Not a cluster index: publishing the hover forces a geometry rebuild, and that rebuild
    /// re-clusters at the live view scale, which renumbers clusters. A stale cluster index would
    /// then grow whatever arrow now sits at that position instead of the one under the cursor. The
    /// direction is part of the identity because a trade's entry and exit share one mark index.
    anchor: (usize, bool),
    /// Indices into the panel's published record list, sorted and deduplicated so two overlapping
    /// clusters sharing a trade list it once.
    trades: Vec<usize>,
}

impl ChartPanel {
    /// Update the hovered arrow from a pointer position.
    ///
    /// `within` is false when the pointer left the chart slot, which clears the hover.
    ///
    /// Args:
    ///     pos: Pointer position in the panel's device pixels.
    ///     within: Whether the pointer is still inside the chart slot.
    ///     cx: Panel context.
    ///
    /// Returns:
    ///     Whether the GPUI tree must repaint.
    pub(super) fn sync_trade_hover(
        &mut self,
        pos: (f32, f32),
        within: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !within {
            return self.clear_trade_hover(cx);
        }
        // Raw pointer events arrive with subpixel jitter, and rescanning for it is wasted work.
        if !super::trade::hover_probe_due(self.trade_hover.probe, pos) {
            return false;
        }
        self.trade_hover.probe = Some(pos);
        let hit = self.trade_hit(pos);
        self.apply_trade_hover(hit, cx)
    }

    /// Drop the hover when the pointer leaves the chart slot entirely.
    ///
    /// Args:
    ///     cx: Panel context used to republish the unhighlighted geometry.
    ///
    /// Returns:
    ///     Whether the GPUI tree must repaint.
    pub(super) fn clear_trade_hover(&mut self, cx: &mut Context<Self>) -> bool {
        self.trade_hover.probe = None;
        self.apply_trade_hover(None, cx)
    }

    /// Re-run the hit test from the last cursor position, without the movement threshold.
    ///
    /// The chart scrolls with the live edge between pointer events, so arrows slide out from under
    /// a resting cursor; the render path calls this so neither a grown arrow nor a card outlives
    /// the trade it belongs to.
    ///
    /// Args:
    ///     cx: Panel context used when the current hit changes the uploaded highlight.
    pub(super) fn revalidate_trade_hover(&mut self, cx: &mut Context<Self>) {
        if self.trade_hover.hover.is_none() {
            return;
        }
        let hit = self.input.cursor.and_then(|pos| self.trade_hit(pos));
        // No notify: this runs INSIDE render, and the card is built from the fresh state right
        // after — a notify here would schedule a repaint on every frame the chart scrolls.
        self.apply_trade_hover(hit, cx);
    }

    /// Hit-test the drawn trade arrows at a panel-local device-pixel position.
    ///
    /// Args:
    ///     pos: Pointer position in panel-local device pixels.
    ///
    /// Returns:
    ///     The resolved arrow and its panel-record indices, or `None` when nothing drawn is hit.
    fn trade_hit(&self, pos: (f32, f32)) -> Option<TradeHover> {
        // Book-only (broom) mode leaves the plot 1 px wide and the engine skips the whole layer
        // there, so without this guard the test would report hits on arrows never drawn.
        if self.orderbook_only {
            return None;
        }
        let pane = self.input.pane_at(pos.0, pos.1)?;
        let map = self.pane_map(pane)?;
        // Arrows exist only over the plot, never in the price-axis gutter or the order-book zone
        // beside it — the same clip the shader applies.
        if pos.0 < map.plot.x
            || pos.0 > map.plot.x + map.plot.w
            || pos.1 < map.plot.y
            || pos.1 > map.plot.y + map.plot.h
        {
            return None;
        }
        let (anchor, trades) = self.chart.with_trade_geometry(pane, |geom| {
            let hit = moon_chart::trade_marks::hit_trade_marks(
                geom.clusters.iter().map(|cluster| TradeMarkAt {
                    x: map.x_of_time(cluster.t_ms),
                    apex_y: map.y_of_price(cluster.price),
                    buy: cluster.buy,
                    count: cluster.members.len() as u32,
                }),
                pos,
                self.last_ppp,
            )?;
            // A cluster's members index the pane's FILTERED mark list; `sources` is the only way
            // back to the panel's records.
            let mut trades = hit
                .stack
                .iter()
                .filter_map(|&cluster| geom.clusters.get(cluster))
                .flat_map(|cluster| cluster.members.iter())
                .filter_map(|&member| geom.sources.get(member).copied())
                .collect::<Vec<_>>();
            trades.sort_unstable();
            trades.dedup();
            // The nearest cluster's first member, paired with its direction, anchors the highlight.
            // Any member would do; the first is the one the sweep seeded the cluster with, so it is
            // stable for as long as the cluster exists at all.
            let cluster = geom.clusters.get(hit.nearest)?;
            let anchor = (cluster.members.first().copied()?, cluster.buy);
            Some((anchor, trades))
        })??;
        (!trades.is_empty()).then_some(TradeHover {
            pane,
            anchor,
            trades,
        })
    }

    /// Apply a hit result and republish the grown arrow.
    ///
    /// Args:
    ///     hit: Resolved hover to publish, or `None` to clear it.
    ///     cx: Panel context used to read the session for the userdata rebuild.
    ///
    /// Returns:
    ///     Whether the GPUI tree must repaint. Unlike the news card, any change repaints because
    ///     this card needs no modifier and is on screen whenever there is a hit.
    fn apply_trade_hover(&mut self, hit: Option<TradeHover>, cx: &mut Context<Self>) -> bool {
        if self.trade_hover.hover == hit {
            return false;
        }
        self.trade_hover.hover = hit;
        let hovered = self
            .trade_hover
            .hover
            .as_ref()
            .map(|hover| (hover.pane, hover.anchor.0, hover.anchor.1));
        if self.chart.set_trade_hover(hovered) {
            // Userdata rebuilds only through `sync_orders_*`; trigger it so the arrow grows now
            // rather than waiting for the next order revision.
            //
            // The ENGINE's method, not the panel's wrapper: the wrapper ends by settling an order
            // drag preview and can `cx.notify()`. This runs from `revalidate_trade_hover` INSIDE
            // render, where a notify schedules another frame — on a live-scrolling chart, every
            // frame. `news.rs` calls the engine directly for exactly this reason.
            let b = self.backend.read(cx);
            self.chart.sync_orders_if_visible(&b.session, true);
        }
        true
    }

    /// The hover card, or `None` when no arrow is hovered.
    ///
    /// Anchored at the CURSOR and clamped inside the slot on BOTH axes. News clamps only x because
    /// its card sits on a fixed axis row; a price-anchored card can be pushed against any edge.
    ///
    /// Args:
    ///     ppp: Device pixels per logical pixel for the current window.
    ///     palette: Active theme colours.
    ///     cx: Application context used for scaled design tokens and translated labels.
    ///
    /// Returns:
    ///     The positioned card, or `None` when no hovered trade can be displayed.
    pub(super) fn trade_hover_card(
        &self,
        ppp: f32,
        palette: MoonPalette,
        cx: &App,
    ) -> Option<AnyElement> {
        let hover = self.trade_hover.hover.as_ref()?;
        let (cursor_x, cursor_y) = self.input.cursor?;
        let ppp = ppp.max(0.1);
        let slot = self.chart.slot_dev_size();
        let slot_w = slot.0 as f32 / ppp;
        let slot_h = slot.1 as f32 / ppp;
        let gap = f32::from(design::ui_px(cx, CARD_GAP));
        // Insets have to fit before they can be honoured: on a pane narrower than two insets, a
        // fixed inset would leave a negative width. The slot always wins over the preferred size.
        let inset = f32::from(design::ui_px(cx, CARD_EDGE_INSET)).min(slot_w * 0.25);
        // The card is a PREFERRED width, never a demanded one. A chart pane can be dragged
        // arbitrarily narrow, and a fixed 268 px card would then hang outside its own slot and over
        // whatever is beside it.
        let card_w = f32::from(design::ui_px(cx, CARD_W)).min((slot_w - inset * 2.0).max(1.0));

        let now_ms = now_unix_ms_i64();
        let rows = self.chart.with_trade_records(|records| {
            let mut shown = hover
                .trades
                .iter()
                .filter_map(|&index| records.get(index))
                .collect::<Vec<_>>();
            // Chronological, so a cluster reads as the sequence it actually traded in. The record
            // order is the query's (newest first) and the indices are sorted, so neither is it.
            shown.sort_by(|left, right| {
                left.buy_date
                    .cmp(&right.buy_date)
                    .then(left.close_date.cmp(&right.close_date))
            });
            let hidden = shown.len().saturating_sub(CARD_MAX_ITEMS);
            let rows = shown
                .into_iter()
                .take(CARD_MAX_ITEMS)
                .map(|record| trade_row(record, now_ms, palette, cx))
                .collect::<Vec<_>>();
            (rows, hidden)
        });
        let (rows, hidden) = rows;
        if rows.is_empty() {
            return None;
        }

        // Device pixels → the slot's logical pixels the overlay is laid out in. Prefer the side of
        // the cursor with room, so the card never covers the arrow the user is pointing at.
        let cursor_x = cursor_x / ppp;
        let cursor_y = cursor_y / ppp;
        let left = if cursor_x + gap + card_w + inset <= slot_w {
            cursor_x + gap
        } else {
            cursor_x - gap - card_w
        }
        // Whichever side was chosen, the card still has to end up inside the slot: on a pane too
        // narrow for either, both branches are out of bounds and only this decides.
        .clamp(inset, (slot_w - card_w - inset).max(inset));
        // The height floor is a PREFERENCE too. `max(CARD_MIN_H)` on its own re-grew the card past
        // a short pane, which is the same mistake as the fixed width: it turns "at least readable"
        // into "at least this tall, slot or no slot".
        let room = (slot_h - inset * 2.0).max(1.0);
        let top = (cursor_y + gap).clamp(inset, (slot_h - inset - CARD_MIN_H.min(room)).max(inset));
        let max_h = (slot_h - top - inset).max(CARD_MIN_H.min(room));
        Some(
            div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(card_w))
                // Never taller than the room below the cursor; a deep stack clips rather than
                // covering the whole chart.
                .max_h(px(max_h))
                .overflow_hidden()
                .shadow_md()
                .child(
                    MoonSurface::new()
                        .variant(MoonSurfaceVariant::Card)
                        // Opaque over a busy chart: the Card variant's faint tint is unreadable
                        // against candles.
                        .bg_color(palette.card)
                        .bg_alpha(1.0)
                        .border_color(palette.border_card)
                        .child(
                            v_flex()
                                .w_full()
                                .gap(design::ui_px(cx, 6.0))
                                .p(design::ui_px(cx, 8.0))
                                .children(rows)
                                .when(hidden > 0, |this| {
                                    this.child(
                                        div()
                                            .text_size(design::t_caption(cx))
                                            .text_color(rgb(palette.text_muted))
                                            .child(
                                                t!("chart.trade_history.hover.more", n = hidden)
                                                    .to_string(),
                                            ),
                                    )
                                }),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// Format one trade's realized result, with the currency it is actually settled in.
///
/// Returns `None` when neither a labelled amount nor a percentage can be rendered. That is an
/// ABSENCE and is rendered as such by the caller: a legacy replica without `profitbtc`, an invalid
/// amount, or an amount without a resolvable currency must never read as a trade that made exactly
/// nothing.
///
/// The ticker is carried on the record rather than derived from the coin, because it is not
/// derivable from the coin — a COIN-M row spells its coin like a USD-M one while settling in BTC.
/// A row whose currency could not be resolved shows the percentage alone, which needs no unit.
///
/// Args:
///     record: Durable trade record carrying optional settled result fields.
///
/// Returns:
///     Display text and its sign, or `None` when no safe result text is available.
fn profit_text(record: &ChartTradeRecord) -> Option<(String, fmt::DeltaSign)> {
    let mut parts: Vec<String> = Vec::new();
    let mut sign = fmt::DeltaSign::Zero;
    // Both halves are required together: an amount whose currency could not be resolved is exactly
    // the number this design refuses to print, so it falls through to the percentage alone.
    if let Some(profit) = record.profit
        && let Some(quote) = record.quote
    {
        let (text, delta) = fmt::signed_amount(profit, quote.display_decimals());
        sign = delta;
        parts.push(format!("{text} {}", quote.ticker()));
    }
    if let Some((text, delta)) = record
        .profit_pct
        .and_then(|v| fmt::signed_pct(v, PCT_DECIMALS))
    {
        // The percentage decides the colour when it is the only figure present, and agrees with
        // the amount when both are: both are computed from the same settled profit.
        if parts.is_empty() {
            sign = delta;
        }
        parts.push(format!("({text})"));
    }
    (!parts.is_empty()).then(|| (parts.join(" "), sign))
}

/// One trade inside the card: side and times on the first line, prices, size and result below.
///
/// Args:
///     record: Durable trade record to render.
///     now_ms: Current Unix time in milliseconds for dated clock formatting.
///     p: Active theme palette.
///     cx: Application context used for scaled design tokens and translations.
///
/// Returns:
///     One complete card-row element.
fn trade_row(record: &ChartTradeRecord, now_ms: i64, p: MoonPalette, cx: &App) -> AnyElement {
    let side_color = if record.is_short { p.red } else { p.green };
    let side = if record.is_short {
        SIDE_SHORT
    } else {
        SIDE_LONG
    };
    // The chart's own axis formatter, so the card and the axis under it cannot disagree about when
    // a trade happened — and it adds the date when the trade is not from today.
    let clock = |seconds: i64| {
        crate::chartdx::axes::format_clock_dated(
            seconds.saturating_mul(1_000) as f64,
            true,
            now_ms as f64,
        )
    };
    let head = h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(rgb(side_color))
                .child(side.to_string()),
        )
        .child(
            div()
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .font_family(design::mono())
                .child(format!(
                    "{} → {}",
                    clock(record.buy_date),
                    clock(record.close_date)
                )),
        );
    let prices = h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .flex_none()
                .text_size(design::t_body(cx))
                .text_color(rgb(p.text))
                .font_family(design::mono())
                .child(format!(
                    "{} → {}",
                    fmt::adaptive(record.buy_price),
                    fmt::adaptive(record.sell_price)
                )),
        )
        .child(
            div()
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .font_family(design::mono())
                .child(format!(
                    "{} {}",
                    t!("chart.trade_history.hover.qty"),
                    fmt::qty(record.quantity)
                )),
        );
    // Profit and loss is the ONE place a profit/loss colour is legitimate here: the chart itself
    // has already spent colour on the side, so this is the only surface left that can carry it.
    let result = match profit_text(record) {
        Some((text, sign)) => div()
            .text_size(design::t_body(cx))
            .text_color(rgb(sign.pick(p.green, p.red, p.text_muted)))
            .font_family(design::mono())
            .child(text),
        None => div()
            .text_size(design::t_caption(cx))
            .text_color(rgb(p.text_muted))
            .child(format!(
                "{}: {}",
                t!("chart.trade_history.hover.pnl"),
                t!("chart.trade_history.hover.pnl_unknown")
            )),
    };
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 2.0))
        .child(head)
        .child(prices)
        .child(result)
        .into_any_element()
}
