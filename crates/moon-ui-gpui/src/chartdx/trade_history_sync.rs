//! Adapter between the durable closed-trade replica and the chart's userdata layer.
//!
//! The geometry itself — which arrow an action gets, what colour a side is drawn in, where the
//! connector runs — lives in [`moon_chart::trade_marks`], because `moon-ui-gpui`'s own `tests/`
//! integration suite cannot import this binary crate's items (hence the static text contracts in
//! `tests/theme_contract/`). A `src/**/tests.rs` sibling unit-test module, like this file's own,
//! compiles and runs normally and is where a private free function belongs. What stays here is
//! only what needs the chart's own state: filtering to the pane's core, rebasing timestamps onto
//! the chart epoch, resolving theme colours, publishing hover state, and retaining the exact
//! cluster snapshot uploaded for hit-testing.

use std::rc::Rc;

use moon_chart::layers::{LineInstance, MarkerInstance, SegInstance, ZoneInstance};
use moon_chart::trade_marks::{self, TradeCluster, TradeMark};
use moon_chart::view::ChartView;
use moon_core::db::ChartTradeRecord;
use moon_core::session::CoreId;

use super::ChartDataState;

/// Publish a complete session-authored userdata union.
pub(crate) fn set(
    layers: &mut super::backend::PlatformLayers,
    zones: Vec<ZoneInstance>,
    hlines: Vec<LineInstance>,
    segs: Vec<SegInstance>,
    markers: Vec<MarkerInstance>,
) {
    layers.set_userdata(&zones, &hlines, &segs, &markers);
}

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
pub(super) fn trade_kind_visible(
    graphics: &moon_core::config::ChartGraphicsCfg,
    emulator: bool,
) -> bool {
    if emulator {
        graphics.show_emulator_trades
    } else {
        graphics.show_real_trades
    }
}

/// Lift a record's core-local entry/exit stamps onto the chart's true-UTC millisecond axis.
///
/// The conversion (correct the seconds part once, re-attach any sub-second remainder) lives
/// inside [`moon_core::db::ReportAxis::stamp_to_utc_ms`]. Multiplying first and subtracting a
/// seconds-offset afterwards is off by a factor of a thousand, and silently so. This cannot be
/// baked into `query_chart_trade_history` instead:
/// `panels/report/trade_detail.rs:104-105` already applies `axis.to_utc` to the record it gets
/// back from that same function, and a corrected column would double-correct it.
///
/// Args:
///     record: Raw closed-trade record, straight from the durable replica.
///     axis: This engine's current report axis.
///
/// Returns:
///     The mark with `buy_ms`/`close_ms` on the chart's true-UTC millisecond epoch.
/// One record as a mark, with a choice of which ends draw their arrows.
///
/// Args:
///     record: The closed trade.
///     axis: The report axis its stamps are lifted through.
///     show_entry: Whether the entry arrow draws.
///     show_exit: Whether the exit arrow draws.
fn trade_mark_with(
    record: &ChartTradeRecord,
    axis: &moon_core::db::ReportAxis,
    show_entry: bool,
    show_exit: bool,
) -> TradeMark {
    let (buy, close) = (record.buy_stamp(), record.close_stamp());
    let (buy_ms, close_ms) = axis.stamp_pair_to_utc_ms(buy, close, record.core_uid);
    TradeMark {
        buy_ms,
        close_ms,
        buy_price: record.buy_price,
        sell_price: record.sell_price,
        qty: record.quantity,
        is_short: record.is_short,
        show_entry,
        show_exit,
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
    /// Mark every pane's trade-history geometry dirty and request a present.
    ///
    /// The shared body of every setter here that invalidates the userdata pass rather than a
    /// single pane: `set_trade_history`, `set_trade_hover`, and `set_report_axis` all need
    /// exactly this.
    fn dirty_all_trade_panes(&mut self) {
        let mut render = self.render.borrow_mut();
        for pane in &mut render.panes {
            pane.last_trade_history_sig = u64::MAX;
            // The archived store is built from the same history over the same axis: it goes
            // with the arrows.
            pane.archived_store = None;
            pane.archived_store_key = None;
            pane.gpu_prepare_dirty = true;
        }
        render.needs_present = true;
    }

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
        self.dirty_all_trade_panes();
        true
    }

    /// Replace the report axis this engine's closed-trade stamps are corrected on.
    ///
    /// Args:
    ///     axis: This engine's current report axis, synced in from `Backend` on render.
    ///
    /// Returns:
    ///     Whether the axis changed.
    pub(super) fn set_report_axis(&mut self, axis: moon_core::db::ReportAxis) -> bool {
        if self.report_axis == axis {
            return false;
        }
        self.report_axis = axis;
        self.dirty_all_trade_panes();
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
        self.dirty_all_trade_panes();
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
        // In the lines style an arriving answer takes a trade's arrows away, so the answers'
        // revision is part of what this layer was built from.
        let lines_rev = if self.draws_trade_lines() {
            self.archived_lines_rev
        } else {
            0
        };
        let sig = self
            .trade_history_revision
            .wrapping_mul(0xD6E8_FEB8_6659_FD93)
            .wrapping_add((self.last_ppp.to_bits() as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9))
            .wrapping_add(colors)
            .wrapping_add(lines_rev.wrapping_mul(0x94D0_49BB_1331_11EB))
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
    ///     lines_drawn: `Some` when this pane's order pass draws the archived lines, so the
    ///         arrows of an answered end must give way; carries the close instants of the live
    ///         closed orders the live pass draws, whose trades get no arrows at all.
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
        lines_drawn: Option<&[f64]>,
    ) -> TradeGeometry {
        if self.orderbook_only {
            return TradeGeometry::default();
        }
        // When the order pass draws the archived lines, an END the archive answered for is
        // drawn as its line, and its arrow would sit on top of it: one or the other, per end.
        // The core archives only a line that was repriced, so a market entry usually comes with
        // an exit line and no entry line; the entry then keeps its arrow. A trade with no lines
        // at all — older than the archive, or not answered yet — keeps both, so an old history is
        // not a blank chart. The CALLER says whether the lines are drawn — a pane without a core
        // runs no order pass at all, and its arrows stay — and names the trades that closed this
        // session, which the live store draws whole: no arrow for either of their ends.
        let epoch_ms = view.epoch_ms;
        let mut sources = Vec::new();
        // The replica stores seconds and, when the core supplied them, milliseconds; every other
        // instance in this layer is relative milliseconds.
        let marks = self
            .trade_history
            .iter()
            .enumerate()
            .filter(|(_, record)| record.core_uid == core)
            .filter(|(_, record)| trade_kind_visible(&self.chart_graphics, record.emulator))
            .filter_map(|(index, record)| {
                let (entry_lined, exit_lined) = match lines_drawn {
                    Some(twins) if self.is_live_twin(record, twins) => (true, true),
                    Some(_) => self.archived_line_ends(record),
                    None => (false, false),
                };
                if entry_lined && exit_lined {
                    return None;
                }
                // Built in the same pass as the marks, so the two lists cannot fall out of step.
                sources.push(index);
                Some(trade_mark_with(
                    record,
                    &self.report_axis,
                    !entry_lined,
                    !exit_lined,
                ))
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
