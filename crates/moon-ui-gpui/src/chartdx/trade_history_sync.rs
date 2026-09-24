//! Adapter between the durable closed-trade replica and the chart's userdata layer.
//!
//! The geometry itself — which arrow an action gets, what colour a side is drawn in, where the
//! connector runs — lives in [`moon_chart::trade_marks`], because `moon-ui-gpui`'s own `tests/`
//! integration suite cannot import this binary crate's items (hence the static text contracts in
//! `tests/theme_contract/`). A `src/**/tests.rs` sibling unit-test module, like this file's own,
//! compiles and runs normally and is where a private free function belongs. What stays here is
//! only what needs the chart's own state: filtering each pane to its own core and, when that pane
//! owns the history request, to the admitted set, rebasing timestamps onto
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
    zones: &[ZoneInstance],
    hlines: &[LineInstance],
    segs: &[SegInstance],
    markers: &[MarkerInstance],
) {
    layers.set_userdata(zones, hlines, segs, markers);
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

/// Whether one closed-trade record may draw on this pane.
///
/// A panel's `trade_history` is one list for one `(core, market)` request, and a multi-pane panel
/// (the Compare kind) draws that same list on every pane. A record whose core is this pane's core
/// draws. A foreign record draws only when this pane owns the request (`owner == pane`) and the
/// record's core is in `admitted`; otherwise a Compare follower would draw the anchor pane's
/// foreign trades.
///
/// With the flag off the admitted set is only the owner, so the list holds only that core's rows
/// and a follower pane draws nothing. With the flag on the list holds every admitted core, and a
/// follower draws its own core's rows from it. `owner == pane` still stops that follower from
/// drawing any other core.
///
/// `None` is own-core only. That is a panel which has not loaded a target yet, and the frozen
/// Trade window, which publishes records but never hands over a core set. A loaded panel stores
/// `Some` even when the flag is off and the set is just the owner; that set draws the same rows
/// `None` would.
///
/// Args:
///     pane: The pane's own core.
///     record: The record's core.
///     cores: The set the panel was handed, or `None` when it never handed one.
///
/// Returns:
///     Whether the record draws on this pane.
fn pane_admits_record(pane: CoreId, record: CoreId, cores: Option<&TradeHistoryCores>) -> bool {
    if record == pane {
        return true;
    }
    matches!(cores, Some(c) if c.owner == pane && c.admitted.contains(&record))
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

/// The cores one panel's trade-history list was loaded for.
///
/// `owner` is the core whose `(core, market)` request loaded that list. `admitted` is the full
/// set the request was made for, the owner first.
#[derive(Debug, PartialEq)]
pub(crate) struct TradeHistoryCores {
    /// Core whose request loaded the panel's record list.
    pub(crate) owner: CoreId,
    /// Every core that request covered, `owner` first.
    pub(crate) admitted: Vec<CoreId>,
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
    /// A pane draws its own core's trades and, when it owns the history request and the panel was
    /// handed an admitted set, that set's trades. Its mark indices — which is what
    /// `TradeCluster::members` holds — stay per pane, so they are not the panel's indices: two
    /// panes filter the same list differently and disagree about what "member 3" means, and the
    /// hover card would show the wrong trades without this map. `sources` still sends those
    /// indices back to the panel's record list. Carrying the map rather than a record id also
    /// sidesteps the legacy rows whose id column collapses to `0`, which cannot tell two trades
    /// apart at all.
    pub sources: Rc<[usize]>,
    /// `clusters` indices ascending by time, so a hit test visits only the arrows near the cursor.
    pub order_by_t: Vec<u32>,
    /// Index of the first arrow in the pane's userdata marker union: cluster `i` is marker
    /// `marker_offset + i`.
    pub marker_offset: u32,
    /// The context the arrows were built with, so a hover can rebuild one instance exactly.
    pub ctx: Option<trade_marks::TradeGeometryCtx>,
    /// Absolute-ms span the arrows were culled to; `None` when nothing was culled. A cursor
    /// outside it is over arrows this geometry never built.
    pub span: Option<(f64, f64)>,
}

/// What a hover change needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TradeHoverChange {
    /// Same arrow as before; nothing to do.
    Unchanged,
    /// The old and new hot arrows were rewritten in place; no rebuild is needed.
    Patched,
    /// Every pane was marked dirty; the caller must run the forced order sync.
    NeedsRebuild,
}

/// A pane's filtered trade marks and their sorted actions, kept across pans and zooms.
///
/// Filtering the record list, exploding it and sorting the actions depends on the trades and the
/// line-drawing inputs, never on the camera, so a pan that rebuilds the arrows reuses all of it.
pub(crate) struct TradeSource {
    /// What the marks were built from; see `append_trade_history_geometry`.
    key: u64,
    marks: Vec<TradeMark>,
    /// Record-list index of each mark, as `TradeGeometry::sources`.
    sources: Rc<[usize]>,
    sorted: trade_marks::SortedActions,
}

/// The time span a pane's trade geometry is built over, keyed so it moves only in coarse steps.
///
/// The visible range `[a - (1 - f) W, a + f W]` (`ChartView::visible_x`, `a` the right time, `W`
/// the window width, `f` the right margin) lies inside `[(cell - 1) Q, (cell + 2) Q]` for
/// `Q = 2^ceil(log2 W) >= W` and `cell = floor(a / Q)`, so a pan rebuilds only when `a` crosses a
/// cell.
///
/// Args:
///     right_time_ms: The view's right time, absolute ms.
///     width_px: The widest the pane's plot can be, device px.
///     px_per_ms: Horizontal view scale, device px per ms.
///
/// Returns:
///     `(cell, log2 Q, span)` with the span in absolute ms, or `None` when the scale is unusable
///     and nothing may be culled.
pub(super) fn built_span(
    right_time_ms: f64,
    width_px: u32,
    px_per_ms: f32,
) -> Option<(i64, i32, (f64, f64))> {
    let window = width_px as f64 / px_per_ms as f64;
    if !(window.is_finite() && window > 0.0 && right_time_ms.is_finite()) {
        return None;
    }
    let log2_q = window.log2().ceil() as i32;
    let q = 2f64.powi(log2_q);
    let cell = (right_time_ms / q).floor();
    Some((cell as i64, log2_q, (cell * q - q, (cell + 2.0) * q)))
}

impl ChartDataState {
    /// Mark every pane's trade-history geometry dirty and request a present.
    ///
    /// The shared body of every setter here that invalidates the userdata pass rather than a
    /// single pane: `set_trade_history`, `set_trade_history_cores`, `set_trade_hover`, and
    /// `set_report_axis` all need exactly this.
    pub(super) fn dirty_all_trade_panes(&mut self) {
        let mut render = self.render.borrow_mut();
        for pane in &mut render.panes {
            pane.last_trade_history_sig = u64::MAX;
            // The archived store is built from the same history over the same axis: it goes
            // with the arrows.
            pane.archived_store = None;
            pane.archived_store_key = None;
            // So is the filtered mark list: the report axis and the history live in its inputs.
            pane.trade_source = None;
            pane.gpu_prepare_dirty = true;
        }
        render.needs_present = true;
    }

    /// Mark one pane's trade arrows for a rebuild over a fresh span, keeping its filtered marks.
    ///
    /// Only the camera-dependent geometry is stale here, so the pane's mark list and archived
    /// store stay; `last_order_sig` is reset because the order signature cannot see this pane's
    /// built span, and an unforced sync then rebuilds only the pane whose gate now mismatches.
    ///
    /// Args:
    ///     pane: Pane index whose geometry no longer covers the cursor.
    pub(super) fn dirty_trade_pane(&mut self, pane: usize) {
        let mut render = self.render.borrow_mut();
        let Some(pr) = render.panes.get_mut(pane) else {
            return;
        };
        pr.last_trade_history_sig = u64::MAX;
        pr.gpu_prepare_dirty = true;
        render.needs_present = true;
        drop(render);
        self.last_order_sig = u64::MAX;
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

    /// Replace the admitted core set the trade-history filter may widen to.
    ///
    /// `None` is own-core only: every panel until one hands a set, and the frozen Trade window
    /// always. A real change bumps `trade_history_revision` and dirties every pane, because
    /// `trade_history_sig` is what decides whether a pane rebuilds its geometry — a set that
    /// changed without moving that signature would leave the old arrows on screen.
    ///
    /// Args:
    ///     cores: The panel's admitted set, or `None` for own-core only.
    ///
    /// Returns:
    ///     Whether the set changed.
    pub(super) fn set_trade_history_cores(&mut self, cores: Option<Rc<TradeHistoryCores>>) -> bool {
        if self.trade_history_cores == cores {
            return false;
        }
        self.trade_history_cores = cores;
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
    /// The hover is qualified by PANE, not merely by mark: a pane draws its own core's trades and,
    /// when it owns the history request and the panel was handed an admitted set, that set's
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
    pub(super) fn set_trade_hover(
        &mut self,
        hovered: Option<(usize, usize, bool)>,
    ) -> TradeHoverChange {
        if self.trade_hovered == hovered {
            return TradeHoverChange::Unchanged;
        }
        let previous = self.trade_hovered;
        self.trade_hovered = hovered;
        if !super::backend::PlatformLayers::can_patch_markers() {
            self.dirty_all_trade_panes();
            return TradeHoverChange::NeedsRebuild;
        }
        // Only DX11 retains the marker buffer to patch; elsewhere the patch declines.
        if self.patch_trade_hover(previous, hovered) {
            return TradeHoverChange::Patched;
        }
        self.dirty_all_trade_panes();
        TradeHoverChange::NeedsRebuild
    }

    /// Rewrite the previously and newly hovered arrows in the uploaded marker buffers.
    ///
    /// Only the arrow instance changes on hover — its size and alpha — so the two instances are
    /// rebuilt through `trade_marker` exactly as the full build makes them.
    ///
    /// Returns:
    ///     Whether both were patched; `false` leaves the caller to rebuild.
    fn patch_trade_hover(
        &self,
        previous: Option<(usize, usize, bool)>,
        hovered: Option<(usize, usize, bool)>,
    ) -> bool {
        let mut render = self.render.borrow_mut();
        let mut touched = false;
        for (pane, mark, buy) in [previous, hovered].into_iter().flatten() {
            let Some(pr) = render.panes.get_mut(pane) else {
                continue;
            };
            let geom = &pr.trade_geometry;
            let Some(ctx) = geom.ctx.as_ref() else {
                return false;
            };
            // Nothing drawn for this action here: nothing to rewrite.
            let Some(ix) = geom
                .clusters
                .iter()
                .position(|c| trade_marks::cluster_is_hot(c, Some((mark, buy))))
            else {
                continue;
            };
            let hot = hovered == Some((pane, mark, buy));
            let marker = trade_marks::trade_marker(&geom.clusters[ix], ctx, hot);
            if !pr
                .layers
                .patch_markers(&[(geom.marker_offset + ix as u32, marker)])
            {
                return false;
            }
            pr.gpu_prepare_dirty = true;
            touched = true;
        }
        if touched {
            render.needs_present = true;
        }
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
            .wrapping_add(trade_marks::scale_bucket(view.px_per_ms, view.px_per_price))
            // The arrows are culled to the built span, so leaving it has to rebuild them.
            .wrapping_add(
                built_span(view.right_time_ms, self.w, view.px_per_ms)
                    .map_or(u64::MAX, |(cell, log2_q, _)| {
                        (cell as u64)
                            .wrapping_mul(0xA24B_AED4_963E_E407)
                            .wrapping_add((log2_q as u64).wrapping_mul(0x9FB2_1C65_1E98_DF25))
                            .wrapping_add(u64::from(self.w))
                    })
                    .wrapping_mul(0xC2B2_AE3D_27D4_EB4F),
            );
        if sig == u64::MAX { 0 } else { sig }
    }

    /// Append entry/exit arrows and their connectors for the trades this pane may draw.
    ///
    /// A pane draws its own core's trades and, when it owns the history request and the panel was
    /// handed an admitted set, that set's trades. Mark indices stay per pane, and `sources` maps
    /// them back to the panel's record list.
    ///
    /// In the Moonbot-lines style an admitted record the pane does not own keeps both arrows.
    /// The archived exit line is built only for the pane's own core, while a lined end loses its
    /// arrow, so treating a foreign trade as lined would hide its exit. Own-core trades stay
    /// lines; foreign admitted trades stay arrows. Widening the archived-line layer is outside
    /// this feature.
    ///
    /// Args:
    ///     pane: Index of the pane being composed, which decides whether it owns the hovered arrow.
    ///     core: The pane's own core. A foreign record draws only when this pane owns the history
    ///         request and the record's core is in the admitted set.
    ///     view: The pane's own view, supplying the epoch and the scale clustering works in.
    ///     markers: Existing order/figure/news marker union to extend.
    ///     lines_drawn: `Some` when this pane's order pass draws the archived lines, so the
    ///         arrows of an answered end must give way; carries the close instants of the live
    ///         closed orders the live pass draws, whose trades get no arrows at all.
    ///     segs: Existing order/figure segment union to extend with the connectors.
    ///     source: The pane's retained `TradeSource`, reused while its key holds.
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
        source: &mut Option<TradeSource>,
    ) -> TradeGeometry {
        if self.orderbook_only {
            return TradeGeometry::default();
        }
        #[cfg(test)]
        TRADE_GEOMETRY_BUILDS.with(|n| n.set(n.get() + 1));
        // Everything the filter below reads besides the history and the report axis, whose
        // setters drop the cache through `dirty_all_trade_panes`.
        let key = self
            .trade_history_revision
            .wrapping_mul(0xD6E8_FEB8_6659_FD93)
            .wrapping_add(self.archived_lines_rev.wrapping_mul(0x94D0_49BB_1331_11EB))
            .wrapping_add(
                self.archived_graphics_bits()
                    .wrapping_mul(0xBF58_476D_1CE4_E5B9),
            )
            .wrapping_add(u64::from(self.draws_live_market()).wrapping_mul(0x9E37_79B9_7F4A_7C15))
            .wrapping_add(
                lines_drawn
                    .map_or(u64::MAX, crate::chartdx::archived_lines::twins_signature)
                    .wrapping_mul(0xA24B_AED4_963E_E407),
            )
            .wrapping_add(core.wrapping_mul(0x9FB2_1C65_1E98_DF25));
        if source.as_ref().is_none_or(|cached| cached.key != key) {
            *source = Some(self.trade_source(core, lines_drawn, key));
        }
        let Some(source) = source.as_ref() else {
            return TradeGeometry::default();
        };
        let window = built_span(view.right_time_ms, self.w, view.px_per_ms).map(|(_, _, span)| {
            let margin = trade_marks::trade_glyph_margin_ms(
                self.last_ppp,
                self.chart_graphics.trade_arrow_scale,
                view.px_per_ms,
            );
            (span.0 - margin, span.1 + margin)
        });
        let marker_offset = markers.len() as u32;
        let ctx = trade_marks::TradeGeometryCtx {
            epoch_ms: view.epoch_ms,
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
        };
        let clusters = moon_chart::trade_marks::build_trade_geometry_sorted(
            &source.marks,
            &source.sorted,
            window,
            &ctx,
            markers,
            segs,
        );
        TradeGeometry {
            order_by_t: trade_marks::order_by_time(&clusters),
            clusters,
            sources: Rc::clone(&source.sources),
            marker_offset,
            ctx: Some(ctx),
            span: window,
        }
    }

    /// Filter the panel's records to the marks this pane draws and sort their actions.
    ///
    /// Args:
    ///     core: The pane's own core.
    ///     lines_drawn: As `append_trade_history_geometry` takes it.
    ///     key: The inputs signature to stamp the result with.
    ///
    /// Returns:
    ///     The pane's retained trade source.
    fn trade_source(&self, core: CoreId, lines_drawn: Option<&[f64]>, key: u64) -> TradeSource {
        // When the order pass draws the closed trades as lines, an END drawn as a line loses its
        // arrow, which would sit on top of it: one or the other, per end. For the pane's OWN core
        // the EXIT is a line — from the archive or from the row itself — so its arrow never draws
        // in that style; the ENTRY is a line only when the core archived its own entry line (it
        // archives only a line its chart gave a point, so a market entry usually has none) and
        // keeps its arrow otherwise. A record the pane admits but does not own is not lined: the
        // archived exit line is never built for a foreign core, and dropping its arrow would hide
        // the exit. The CALLER says whether the lines are drawn — a pane without a core runs no
        // order pass at all, and its arrows stay — and names the trades that closed this session,
        // which the live store draws whole: no arrow for either of their ends.
        let mut sources = Vec::new();
        // The replica stores seconds and, when the core supplied them, milliseconds; every other
        // instance in this layer is relative milliseconds.
        let marks = self
            .trade_history
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                pane_admits_record(core, record.core_uid, self.trade_history_cores.as_deref())
            })
            .filter(|(_, record)| trade_kind_visible(&self.chart_graphics, record.emulator))
            .filter_map(|(index, record)| {
                // On a live chart a twin of a live closed order draws both its lines from the
                // session store; on a frozen viewer the list names what the frozen store draws,
                // and the ends are decided per trade — see `archived_line_ends`. A record this
                // pane admits but does not own never reaches that: it keeps both arrows.
                let owned = record.core_uid == core;
                let (entry_lined, exit_lined) = if owned {
                    match lines_drawn {
                        Some(twins)
                            if self.draws_live_market() && self.is_live_twin(record, twins) =>
                        {
                            (true, true)
                        }
                        Some(drawn) => self.archived_line_ends(record, drawn),
                        None => (false, false),
                    }
                } else {
                    (false, false)
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
        let sorted = trade_marks::sort_actions(&trade_marks::explode_actions(&marks));
        TradeSource {
            key,
            marks,
            sources: sources.into(),
            sorted,
        }
    }
}

#[cfg(test)]
thread_local! {
    static TRADE_GEOMETRY_BUILDS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Trade-geometry builds on this thread since the last call; resets the count.
#[cfg(test)]
#[allow(dead_code)] // read by the prover's before/after measurement
pub(crate) fn take_trade_geometry_builds() -> u64 {
    TRADE_GEOMETRY_BUILDS.with(|n| n.replace(0))
}

#[cfg(test)]
mod tests;
