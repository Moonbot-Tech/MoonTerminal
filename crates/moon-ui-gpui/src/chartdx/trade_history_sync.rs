//! Adapter between the durable closed-trade replica and the chart's userdata layer.
//!
//! The geometry itself — which arrow an action gets, what colour a side is drawn in, where the
//! connector runs — lives in [`moon_chart::trade_marks`], because `moon-ui-gpui` is a binary crate
//! whose logic no test can import. What stays here is only what needs the chart's own state:
//! filtering to the pane's core, rebasing timestamps onto the chart epoch, resolving theme colours,
//! publishing hover state, and retaining the exact cluster snapshot uploaded for hit-testing.

use std::rc::Rc;

use moon_chart::layers::{MarkerInstance, SegInstance};
use moon_chart::trade_marks::{self, TradeCluster, TradeMark};
use moon_chart::view::ChartView;
use moon_core::db::ChartTradeRecord;
use moon_core::session::CoreId;

use super::ChartDataState;

/// Whether a closed trade of this kind is drawn, per the graphics popup's two checkboxes.
///
/// The ONE definition of the real/emulator trade filter, and it lives at DRAWING time rather than in
/// the durable query on purpose. Narrowing the SQL instead would let a display toggle decide which
/// rows the history CONTAINS: the row cap is applied after the predicate, so hiding emulator trades
/// would free slots under it and surface older REAL trades that had been truncated away, and the
/// on-screen trade count would answer to a checkbox. Here the query is one thing and the drawing
/// another — unticking both empties the layer without touching the database.
///
/// Args:
///     graphics: The chart's graphics settings.
///     emulator: Whether an emulator order made the trade.
///
/// Returns:
///     Whether the trade's marks are drawn.
fn trade_kind_visible(graphics: &moon_core::config::ChartGraphicsCfg, emulator: bool) -> bool {
    if emulator {
        graphics.show_emulator_trades
    } else {
        graphics.show_real_trades
    }
}

/// Everything a pane must retain about the trade arrows it currently has on the GPU.
///
/// The two halves travel together because neither is usable alone: the clusters say WHERE the
/// arrows are and how many trades each stands for, and `sources` is the only way back from a
/// cluster's members to the panel's own record list. They are produced by one call and stored in
/// one field so a future edit cannot refresh one and leave the other describing a previous build.
#[derive(Default)]
pub(crate) struct TradeGeometry {
    /// The clusters the uploaded arrows were built from, one per drawn marker.
    pub clusters: Vec<TradeCluster>,
    /// For each entry of this pane's FILTERED mark list, its index in the panel's record list.
    ///
    /// A pane draws only the trades of its OWN core, so its mark indices — which is what
    /// `TradeCluster::members` holds — are not the panel's indices. Two panes on two cores
    /// therefore disagree about what "member 3" means, and the hover card would show the wrong
    /// trades without this map. Carrying the map rather than a record id also sidesteps the legacy
    /// rows whose id column collapses to `0`, which cannot tell two trades apart at all.
    pub sources: Vec<usize>,
}

impl ChartDataState {
    /// Replace the exact-target durable history and invalidate userdata only on a real change.
    ///
    /// Args:
    ///     records: Exact-target durable history owned by the chart panel.
    ///
    /// Returns:
    ///     Whether the record set changed.
    pub(super) fn set_trade_history(&mut self, records: Rc<Vec<ChartTradeRecord>>) -> bool {
        if Rc::ptr_eq(&self.trade_history, &records) || self.trade_history == records {
            return false;
        }
        self.trade_history = records;
        self.trade_history_revision = self.trade_history_revision.wrapping_add(1);
        let mut render = self.render.borrow_mut();
        for pane in &mut render.panes {
            pane.last_trade_history_sig = u64::MAX;
            pane.gpu_prepare_dirty = true;
        }
        render.needs_present = true;
        true
    }

    /// Replace the hovered arrow and invalidate userdata only on a real change.
    ///
    /// The hover is qualified by PANE, not merely by mark: each pane draws only its own core's
    /// trades, so a bare index names a different trade on every pane and would grow an unrelated
    /// marker on all the others. It is also a MARK rather than a cluster index, so that the
    /// rebuild this very call triggers cannot move the highlight onto a neighbouring arrow — see
    /// `TradeGeometryCtx::hovered`.
    ///
    /// Args:
    ///     hovered: Pane index, a mark index within that pane, and whether the hovered end BUYS.
    ///
    /// Returns:
    ///     Whether the hovered arrow changed.
    pub(super) fn set_trade_hover(&mut self, hovered: Option<(usize, usize, bool)>) -> bool {
        if self.trade_hovered == hovered {
            return false;
        }
        self.trade_hovered = hovered;
        let mut render = self.render.borrow_mut();
        for pane in &mut render.panes {
            pane.last_trade_history_sig = u64::MAX;
            pane.gpu_prepare_dirty = true;
        }
        render.needs_present = true;
        true
    }

    /// Return a non-sentinel signature for the current durable-history geometry.
    ///
    /// Folds in the device scale and both side colours beside the revision, exactly as `news_sig`
    /// does and for the same reason: marker sizes are baked in PHYSICAL pixels and colours are
    /// baked per instance, so a DPI change or a theme edit alters the geometry without touching a
    /// single record. Hashing the revision alone left the arrows at the previous scale and colour
    /// until the next trade was replicated.
    ///
    /// The hovered arrow is deliberately NOT folded in: [`Self::set_trade_hover`] already stamps
    /// every pane dirty, exactly as `set_news_marks` does, so this signature covers only the inputs
    /// that reach the instances without passing through a setter.
    ///
    /// Args:
    ///     view: The pane's own view, whose scale decides which trades cluster together.
    ///
    /// Returns:
    ///     Current geometry signature, with the forced-dirty sentinel mapped to zero.
    pub(super) fn trade_history_sig(&self, view: &ChartView) -> u64 {
        let pack = |c: [u8; 3]| ((c[0] as u64) << 16) | ((c[1] as u64) << 8) | c[2] as u64;
        let colors = pack(self.theme.label_positive)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(pack(self.theme.label_negative));
        let sig = self
            .trade_history_revision
            .wrapping_mul(0xD6E8_FEB8_6659_FD93)
            .wrapping_add((self.last_ppp.to_bits() as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9))
            .wrapping_add(colors)
            // Clustering happens when this layer is rebuilt, so ZOOM has to invalidate it — but
            // through a quantized bucket, never the raw scale, or a smooth zoom would rebuild every
            // marker on every frame.
            .wrapping_add(trade_marks::scale_bucket(view.px_per_ms, view.px_per_price));
        if sig == u64::MAX { 0 } else { sig }
    }

    /// Append entry/exit arrows and their connectors for records owned by this exact pane core.
    ///
    /// Args:
    ///     pane: Index of the pane being composed, which decides whether it owns the hovered arrow.
    ///     core: Exact pane core; records from other cores are ignored.
    ///     view: The pane's own view, supplying the epoch and the scale clustering works in.
    ///     markers: Existing order/figure/news marker union to extend.
    ///     segs: Existing order/figure segment union to extend with the connectors.
    ///
    /// Returns:
    ///     The cluster snapshot the markers were built from plus the map back to the panel's
    ///     records, for the pane to retain — hit-testing must read THAT rather than re-cluster,
    ///     since the rebuild signature quantizes the scale and the view keeps moving inside one
    ///     bucket. Empty when the pane is orderbook-only, which draws no trade history at all.
    pub(super) fn append_trade_history_geometry(
        &self,
        pane: usize,
        core: CoreId,
        view: &ChartView,
        markers: &mut Vec<MarkerInstance>,
        segs: &mut Vec<SegInstance>,
    ) -> TradeGeometry {
        if self.orderbook_only {
            return TradeGeometry::default();
        }
        let epoch_ms = view.epoch_ms;
        let mut sources = Vec::new();
        // The replica stores seconds; every other instance in this layer is relative milliseconds.
        let marks = self
            .trade_history
            .iter()
            .enumerate()
            .filter(|(_, record)| record.core_uid == core)
            .filter(|(_, record)| trade_kind_visible(&self.chart_graphics, record.emulator))
            .map(|(index, record)| {
                // Built in the same pass as the marks, so the two lists cannot fall out of step.
                sources.push(index);
                TradeMark {
                    buy_ms: record.buy_date.saturating_mul(1_000),
                    close_ms: record.close_date.saturating_mul(1_000),
                    buy_price: record.buy_price,
                    sell_price: record.sell_price,
                    qty: record.quantity,
                    is_short: record.is_short,
                }
            })
            .collect::<Vec<_>>();
        let clusters = moon_chart::build_trade_geometry(
            &marks,
            &trade_marks::TradeGeometryCtx {
                epoch_ms,
                long_rgb: self.theme.label_positive,
                short_rgb: self.theme.label_negative,
                scale: self.last_ppp,
                px_per_ms: view.px_per_ms,
                px_per_price: view.px_per_price,
                arrow_scale: self.chart_graphics.trade_arrow_scale,
                connector_thickness: self.chart_graphics.connector_thickness_px,
                hovered: self
                    .trade_hovered
                    .and_then(|(hot, mark, buy)| (hot == pane).then_some((mark, buy))),
            },
            markers,
            segs,
        );
        TradeGeometry { clusters, sources }
    }
}

#[cfg(test)]
mod tests;
