//! The trade pane under the deal table: the selected deal drawn as its trade window draws it —
//! the same view (`trade_window::Host::Embedded`), not a second chart — with the trades the
//! variant column would have made beside the fact, dashed: the path the variant's entry order
//! walked, its fill, the path its sell order walked to the exit, and — under the window's
//! MoonShot zone switch — the corridor the model held around the entry order, placement by
//! placement.
//!
//! Folded by default, behind a rail like the one between the halves of the tab; folded, a click
//! on a row selects nothing and nothing is built. Open, a click on a row shows that deal; a
//! double-click still opens its window.
//!
//! The modelled trades are one replay per touched variant on the selected deal's tape — the same
//! parameters and the same tape cut the columns are scored on (`search::variant_picture`) — run
//! whenever the pane shows a new deal and whenever the columns are rescored, for the deals of
//! the sample only (`DealRow::fit`): a trade the model does not reproduce has no variant to
//! draw, as it has no plan in the table.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_chart::frozen_overlay::{OverlayBand, OverlayTrade};
use moon_chart::layers::SEG_PATTERN_DASH;
use moon_core::db::tuner::ticks::ExitKind;
use moon_core::db::tuner::ticks::search::{clip_to_horizon, variant_picture};
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::super::shared::collapse_caret;
use crate::design;
use crate::design::moon;
use crate::trade_window::TradeWindowView;

/// The pane's state, in `TicksState`.
#[derive(Default)]
pub(in crate::analytics::tuner) struct TradePane {
    /// Whether the pane is open; the layout remembers it (`TicksAxisLayout::trade_open`).
    pub(in crate::analytics::tuner) open: bool,
    /// The deal the pane shows, by `ReportUID` — kept while the pane is folded, so opening it
    /// again brings the same deal back.
    pub(in crate::analytics::tuner) uid: Option<i64>,
    /// The view drawing it: `None` while the deal resolves, and while the pane is folded — a
    /// folded pane holds no chart.
    view: Option<Entity<TradeWindowView>>,
    /// The replica could not resolve the deal.
    missing: bool,
    /// Generation of the resolve in flight; an older answer is dropped.
    seq: u64,
    /// Generation of the modelled-trades replay in flight; an older answer is dropped.
    model_seq: u64,
}

/// The pen of the variant's modelled trade: dashed — the fact keeps its solid lines.
const VARIANT_PATTERN: f32 = SEG_PATTERN_DASH;

impl AnalyticsView {
    /// Fold or open the pane, and remember it. Folding drops the chart; opening shows the deal
    /// the pane showed last, if any.
    pub(in crate::analytics::tuner) fn ticks_toggle_trade_pane(&mut self, cx: &mut Context<Self>) {
        let pane = &mut self.ticks.trade;
        pane.open = !pane.open;
        pane.seq = pane.seq.wrapping_add(1);
        pane.model_seq = pane.model_seq.wrapping_add(1);
        pane.view = None;
        pane.missing = false;
        if let Some(uid) = pane.uid.filter(|_| pane.open) {
            self.ticks_show_deal(uid, cx);
        }
        self.persist_ticks_settings(cx);
        cx.notify();
    }

    /// A click on a row: show that deal in the pane, while it is open. A folded pane ignores it.
    pub(in crate::analytics::tuner) fn ticks_select_deal(
        &mut self,
        uid: i64,
        cx: &mut Context<Self>,
    ) {
        let pane = &self.ticks.trade;
        if !pane.open || (pane.uid == Some(uid) && (pane.view.is_some() || !pane.missing)) {
            return;
        }
        self.ticks.trade.uid = Some(uid);
        self.ticks_show_deal(uid, cx);
        cx.notify();
    }

    /// Resolve one deal off the replica and build the pane's view on it.
    fn ticks_show_deal(&mut self, uid: i64, cx: &mut Context<Self>) {
        let pane = &mut self.ticks.trade;
        pane.seq = pane.seq.wrapping_add(1);
        pane.model_seq = pane.model_seq.wrapping_add(1);
        pane.view = None;
        pane.missing = false;
        let seq = pane.seq;
        let Some((target, axis)) = self.deal_target(uid) else {
            self.ticks.trade.missing = true;
            return;
        };
        let weak = cx.entity().downgrade();
        crate::trade_window::open_record::resolve_trade_record(
            axis,
            target,
            cx,
            move |seed, app| {
                let _ = weak.update(app, |this, cx| {
                    let pane = &mut this.ticks.trade;
                    if pane.seq != seq || !pane.open {
                        return;
                    }
                    match seed {
                        Some(seed) => {
                            let backend = this.backend.clone();
                            this.ticks.trade.view =
                                Some(crate::trade_window::embedded_trade_view(&backend, seed, cx));
                            this.ticks_refresh_model_trades(cx);
                        }
                        None => pane.missing = true,
                    }
                    cx.notify();
                });
            },
        );
    }

    /// Replay the variant on the pane's deal and hand its trade to the view. Nothing to replay —
    /// no view, no tape in memory for the deal, an untouched variant — hands it none.
    pub(in crate::analytics::tuner) fn ticks_refresh_model_trades(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.ticks.trade.view.clone() else {
            return;
        };
        self.ticks.trade.model_seq = self.ticks.trade.model_seq.wrapping_add(1);
        let seq = self.ticks.trade.model_seq;
        let changes = self.ticks.variant_changes();
        let job = self.ticks.data.data().and_then(|data| {
            let uid = self.ticks.trade.uid?;
            let row = data
                .rows
                .iter()
                .find(|r| r.deal.report_uid == uid)
                .filter(|r| r.fit())?;
            if changes.is_empty() {
                return None;
            }
            Some((
                data.prepared(row)?,
                data.exit_horizon_ms(),
                data.single_kind().unwrap_or_default().to_string(),
            ))
        });
        let Some((pending, horizon_ms, kind)) = job else {
            view.update(cx, |view, cx| {
                view.set_model_trades(Vec::new(), Vec::new(), cx)
            });
            return;
        };
        let defaults = self.filter_defaults(cx);
        let model = super::model_cfg::current();
        let is_short = pending.deal.is_short;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let picture = executor
                .spawn(async move {
                    // Unpacked here, off the UI thread, and cut as the columns cut it, so the
                    // picture shows what the column counted.
                    let mut deal = pending.prepare();
                    if let Some(horizon_ms) = horizon_ms {
                        clip_to_horizon(std::slice::from_mut(&mut deal), horizon_ms);
                    }
                    variant_picture(&deal, &defaults, &kind, &changes, model)
                })
                .await;
            let mut corridor: Vec<OverlayBand> = Vec::new();
            let trades: Vec<OverlayTrade> = Some(picture)
                .into_iter()
                .filter_map(|picture| {
                    let outcome = picture.outcome;
                    let fill = outcome.fill?;
                    // Each placement's corridor until the next placement, the last one until the
                    // fill.
                    let steps = &picture.corridor;
                    for (i, step) in steps.iter().enumerate() {
                        let to_ms = steps.get(i + 1).map_or(fill.t_ms, |next| next.t_ms);
                        if step.t_ms < fill.t_ms {
                            corridor.push(OverlayBand {
                                from_ms: step.t_ms as f64,
                                to_ms: to_ms.min(fill.t_ms) as f64,
                                prices: (step.band.0 as f32, step.band.1 as f32),
                            });
                        }
                    }
                    Some(OverlayTrade {
                        path: steps
                            .iter()
                            .map(|step| (step.t_ms as f64, step.level as f32))
                            .collect(),
                        fill_ms: fill.t_ms as f64,
                        fill_price: fill.price as f32,
                        exit: outcome
                            .exit
                            .filter(|exit| exit.kind != ExitKind::OpenAtWindowEnd)
                            .map(|exit| (exit.t_ms as f64, exit.price as f32)),
                        exit_path: picture
                            .sell_line
                            .iter()
                            .map(|point| (point.t_ms as f64, point.price as f32))
                            .collect(),
                        is_short,
                        pattern: VARIANT_PATTERN,
                    })
                })
                .collect();
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.ticks.trade.model_seq != seq {
                        return;
                    }
                    view.update(cx, |view, cx| view.set_model_trades(trades, corridor, cx));
                });
            });
        })
        .detach();
    }

    /// The rail that folds the pane: the same caret the rail between the halves carries, laid
    /// across the column, with what the pane draws in words.
    pub(in crate::analytics::tuner) fn ticks_trade_rail(
        &self,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let open = self.ticks.trade.open;
        h_flex()
            .w_full()
            .flex_none()
            .h(design::ui_px(cx, 16.0))
            .items_center()
            .justify_center()
            .gap(design::ui_px(cx, 6.0))
            .child(collapse_caret(
                "an-ticks-trade-collapse",
                !open,
                t!("analytics.ticks.trade_collapse").to_string(),
                t!("analytics.ticks.trade_expand").to_string(),
                p,
                cx.listener(|this, _, _, cx| this.ticks_toggle_trade_pane(cx)),
            ))
            .when(open, |el| {
                el.child(
                    div()
                        .font_family(design::ui_font())
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(t!("analytics.ticks.trade_legend").to_string()),
                )
            })
            .into_any_element()
    }

    /// The open pane: the deal's view, or what stands in for it.
    pub(in crate::analytics::tuner) fn ticks_trade_pane(
        &self,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let pane = &self.ticks.trade;
        let body = match (&pane.view, pane.uid) {
            (Some(view), _) => div().size_full().child(view.clone()).into_any_element(),
            (None, uid) => {
                let key = match uid {
                    None => "analytics.ticks.trade_pick",
                    Some(_) if pane.missing => "analytics.ticks.trade_missing",
                    Some(_) => "analytics.ticks.trade_loading",
                };
                crate::load_state::muted(t!(key).to_string(), 10.0, p, cx)
            }
        };
        v_flex()
            .w_full()
            .flex_1()
            .min_h_0()
            .rounded(design::ui_px(cx, 8.0))
            .border_1()
            .border_color(moon(p.border))
            .overflow_hidden()
            .child(body)
            .into_any_element()
    }
}
