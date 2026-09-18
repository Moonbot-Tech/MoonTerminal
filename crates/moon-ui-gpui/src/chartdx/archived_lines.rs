//! Closed trades drawn as Moonbot's order lines on a LIVE chart.
//!
//! The "Trades: Moonbot lines" style replaces the entry/exit arrows with what the core archived
//! when each trade finalized — its buy and sell lines with their repricing paths and stop markers
//! — drawn through the very geometry a live closed order takes. The panel resolves the lines
//! (`backend::traces`, the process's one resolver) and hands this engine a map by `ReportUID`;
//! this module turns that map plus the pane's own trade history into an `OrderLineStore` the
//! order pass draws as a SECOND source beside the session's live store, never in its place.
//!
//! The EXIT line never waits for the archive: the report row already states where the exit order
//! was placed, at what price and when it filled, so every closed trade draws its exit as a line
//! from the first frame, and the core's archived exit — with its repricing path — replaces that
//! straight line when it arrives. Only the ENTRY still depends on the archive: the report dates
//! the entry's completion, not its placement, so an entry the core archived no line for keeps
//! its arrow until the wire says where the buy line began.
//!
//! Which trades to resolve is this engine's call too: the ones nearest the pane's right edge,
//! because a market's history runs to a thousand rows and the core is asked about at most a few
//! dozen at a time. The panel asks, the engine says what for.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use moon_core::config::TradeHistoryStyle;
use moon_core::db::ChartTradeRecord;
use moon_core::feed::{ArchivedLineKind, ArchivedOrderTrace};
use moon_core::session::CoreId;
use moon_core::session::order_lines::{ArchivedOrdersInput, OrderLineStore, ReportExit};

use super::ChartDataState;
use super::trade_history_sync::trade_kind_visible;

impl ChartDataState {
    /// Whether the effective graphics draw closed trades as lines rather than arrows.
    pub(super) fn draws_trade_lines(&self) -> bool {
        self.chart_graphics.trade_history_style == TradeHistoryStyle::MoonbotLines
    }

    /// The graphics inputs the archived store is shaped by, packed for the per-pane cache key:
    /// the style and the two trade-kind switches.
    pub(super) fn archived_graphics_bits(&self) -> u64 {
        (self.draws_trade_lines() as u64)
            | ((self.chart_graphics.show_real_trades as u64) << 1)
            | ((self.chart_graphics.show_emulator_trades as u64) << 2)
    }

    /// Which ends of this trade the lines pass draws — `(entry, exit)` — so the arrows pass
    /// draws the arrow of the end that has none. The exit is always a line, from the archive or
    /// from the row (see the module doc); the entry only when the core archived its own line —
    /// it archives only a line its chart gave a point, so a market entry typically has none.
    pub(super) fn archived_line_ends(&self, record: &ChartTradeRecord) -> (bool, bool) {
        line_ends(record, &self.archived_lines)
    }

    /// Replace the resolved lines and wake the order pass.
    ///
    /// Args:
    ///     lines: Archived lines by `ReportUID`, one entry per trade the resolver answered with
    ///         lines. A trade without an entry still draws its exit line, from its row.
    ///
    /// Returns:
    ///     Whether the map changed.
    pub(super) fn set_archived_lines(
        &mut self,
        lines: Rc<HashMap<i64, Arc<[ArchivedOrderTrace]>>>,
    ) -> bool {
        if Rc::ptr_eq(&self.archived_lines, &lines) {
            return false;
        }
        self.archived_lines = lines;
        self.archived_lines_rev = self.archived_lines_rev.wrapping_add(1);
        self.mark_view_dirty();
        true
    }

    /// The closed trades worth resolving now: the ones whose exit is nearest a pane's right edge.
    /// See [`rank_wanted`].
    ///
    /// Ranks whatever the style: the panel decides whether to ask, from ITS effective graphics —
    /// this engine's copy lags a style flip until the next render, and the flip is exactly when
    /// the first ask has to go out.
    ///
    /// Args:
    ///     cap: Most uids to return.
    pub(super) fn wanted_trace_uids(&self, cap: usize) -> Vec<i64> {
        if cap == 0 {
            return Vec::new();
        }
        let container = self.container.borrow();
        let edges: Vec<(CoreId, f64)> = container
            .panes()
            .iter()
            .map(|pane| (pane.core, pane.view.right_time_ms))
            .collect();
        rank_wanted(
            &self.trade_history,
            &edges,
            &self.chart_graphics,
            &self.report_axis,
            cap,
        )
    }

    /// The archived store one pane draws, or `None` when the style is arrows or the pane has no
    /// closed trade to draw. See [`archived_store`].
    ///
    /// Args:
    ///     core: The pane's core.
    ///     market: The pane's exchange-native market, which is what the draw side files under.
    ///     live: The session's live store for the core, whose closed orders already draw the
    ///         trades that closed this session.
    pub(super) fn archived_store_for_pane(
        &self,
        core: CoreId,
        market: &str,
        live_closed_ms: &[f64],
    ) -> Option<OrderLineStore> {
        if !self.draws_trade_lines() {
            return None;
        }
        let mut store = archived_store(
            &self.trade_history,
            &self.archived_lines,
            core,
            market,
            live_closed_ms,
            &self.chart_graphics,
            &self.report_axis,
        )?;
        store.rev = self.archived_lines_rev.max(1);
        Some(store)
    }

    /// Close instants of the live closed orders the live pass DRAWS on this market — the trades
    /// that closed this session and still draw both their lines from the session store. Past the
    /// tab's closed-order cap the live line is gone, and the archive has to take over.
    ///
    /// Args:
    ///     market: The pane's market.
    ///     live: The session's live store for the pane's core.
    pub(super) fn live_twin_closes(&self, market: &str, live: &OrderLineStore) -> Vec<f64> {
        live.market_draw_orders(market, self.orders.max_closed_orders as usize)
            .into_iter()
            .filter_map(|order| order.closed_ms)
            .collect()
    }

    /// Whether a record's close sits within [`LIVE_TWIN_TOLERANCE_MS`] of a live closed order's:
    /// the same trade, drawn — both lines — by the live store.
    pub(super) fn is_live_twin(&self, record: &ChartTradeRecord, live_closed_ms: &[f64]) -> bool {
        let (_, close_ms) = record_utc_ms(record, &self.report_axis);
        is_live_twin(close_ms, live_closed_ms)
    }
}

/// See [`ChartDataState::archived_line_ends`]; `lines` is the resolver's map by `ReportUID`.
fn line_ends(
    record: &ChartTradeRecord,
    lines: &HashMap<i64, Arc<[ArchivedOrderTrace]>>,
) -> (bool, bool) {
    // The same test `ReportExit::of_record` makes: a row with no exit price places no line.
    let exit = record.sell_price > 0.0;
    let entry = record
        .report_uid
        .and_then(|uid| lines.get(&uid))
        .is_some_and(|lines| {
            lines
                .iter()
                .any(|line| line.own && line.kind == ArchivedLineKind::Entry)
        });
    (entry, exit)
}

/// One number for the live twins a store was built against, for the per-pane cache key: the
/// live store's revision moves on every order message of the core, the set of closed instants
/// only when an order closes or leaves the ring — and only the latter reshapes the store.
pub(super) fn twins_signature(live_closed_ms: &[f64]) -> u64 {
    live_closed_ms.iter().fold(0u64, |acc, ms| {
        acc.wrapping_mul(0x100_0000_01b3).wrapping_add(ms.to_bits())
    })
}

/// See [`ChartDataState::is_live_twin`].
fn is_live_twin(close_ms: i64, live_closed_ms: &[f64]) -> bool {
    live_closed_ms
        .iter()
        .any(|live| (live - close_ms as f64).abs() <= LIVE_TWIN_TOLERANCE_MS)
}

/// How far apart a report row's close and a live closed order's close may sit and still be one
/// trade. The row's stamp is the core's report clock lifted through the axis, the order's is the
/// wire's own instant; they agree to a second or two, and no market closes two trades this close
/// together often enough to matter — and when it does, the live twin still draws the lines.
const LIVE_TWIN_TOLERANCE_MS: f64 = 15_000.0;

/// Rank the closed trades by how far their exit sits from a pane's right edge and name the
/// first `cap`, nearest first.
///
/// Every pane contributes its own core's trades against its own edge, so the resolver's ask goes
/// to what is on screen or about to be. Trades without a `ReportUID` cannot be asked about and
/// are skipped, as are the kinds the tab hides. A uid appears once however many panes rank it.
///
/// Args:
///     records: The chart's durable history.
///     edges: `(core, right_time_ms)` per pane.
///     graphics: The tab's effective graphics, for the trade-kind switches.
///     axis: The engine's report axis, which lifts the exit onto the chart's UTC.
///     cap: Most uids to return.
fn rank_wanted(
    records: &[ChartTradeRecord],
    edges: &[(CoreId, f64)],
    graphics: &moon_core::config::ChartGraphicsCfg,
    axis: &moon_core::db::ReportAxis,
    cap: usize,
) -> Vec<i64> {
    // Lifted ONCE per record: the axis walk is the expensive half, and every pane's edge ranks
    // the same instants.
    let askable: Vec<(CoreId, i64, f64)> = records
        .iter()
        .filter(|record| trade_kind_visible(graphics, record.emulator))
        .filter_map(|record| {
            let uid = record.report_uid?;
            let (_, close_ms) = record_utc_ms(record, axis);
            Some((record.core_uid, uid, close_ms as f64))
        })
        .collect();
    let mut ranked: Vec<(f64, i64)> = Vec::new();
    for &(core, right_ms) in edges {
        for &(record_core, uid, close_ms) in &askable {
            if record_core == core {
                ranked.push(((close_ms - right_ms).abs(), uid));
            }
        }
    }
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<i64> = Vec::with_capacity(cap.min(ranked.len()));
    for (_, uid) in ranked {
        if out.len() >= cap {
            break;
        }
        if !out.contains(&uid) {
            out.push(uid);
        }
    }
    out
}

/// Every trade of `core` the tab admits, as bright closed orders on `market` — the same builder
/// the trade window's neighbours take. Each gets whatever the resolver answered with, and its
/// exit line from the row where the answer holds none (or has not come yet). `None` when no
/// trade qualifies, so the caller draws nothing extra rather than an empty store. The caller
/// stamps `rev`.
///
/// A trade whose order closed THIS session is still in the live store's closed ring and draws
/// from there as any closed order does — under the tab's closed-order settings, with the local
/// repricing history the archive lacks; its archived twin is skipped (matched by close instant,
/// [`LIVE_TWIN_TOLERANCE_MS`]) so nothing is drawn twice. Once the ring evicts the order, the
/// cap hides it, or on the next start, the archive takes over. Known gap: an order the store
/// closed by its own backstop after a feed gap carries the last-seen instant, minutes off the
/// row's, and draws twice until the ring lets it go.
///
/// Args:
///     records: The chart's durable history.
///     lines: Resolved lines by `ReportUID`.
///     core: The pane's core.
///     market: The pane's exchange-native market.
///     live_closed_ms: Close instants of the live store's closed orders on this market.
///     graphics: The tab's effective graphics, for the trade-kind switches.
///     axis: The engine's report axis.
fn archived_store(
    records: &[ChartTradeRecord],
    lines: &HashMap<i64, Arc<[ArchivedOrderTrace]>>,
    core: CoreId,
    market: &str,
    live_closed_ms: &[f64],
    graphics: &moon_core::config::ChartGraphicsCfg,
    axis: &moon_core::db::ReportAxis,
) -> Option<OrderLineStore> {
    let mut store = OrderLineStore::default();
    let mut drawn = 0usize;
    for record in records {
        if record.core_uid != core || !trade_kind_visible(graphics, record.emulator) {
            continue;
        }
        let (buy_ms, close_ms) = record_utc_ms(record, axis);
        if is_live_twin(close_ms, live_closed_ms) {
            continue;
        }
        // No answer yet, or an empty one: the builder still draws the row's own exit line.
        let archived: &[ArchivedOrderTrace] = record
            .report_uid
            .and_then(|uid| lines.get(&uid))
            .map_or(&[], |lines| lines.as_ref());
        store.append_archived(
            ArchivedOrdersInput {
                market,
                is_short: record.is_short,
                quantity: record.quantity as f32,
                entry_fill_ms: Some(buy_ms as f64),
                close_ms: close_ms as f64,
                exit: ReportExit::of_record(record, axis),
                // Moonbot draws its closed trades' lines in full colour; so does this style.
                bright: true,
            },
            archived,
        );
        drawn += 1;
    }
    (drawn > 0).then_some(store)
}

/// A record's entry and exit lifted onto true UTC through the report axis — the same lift its
/// arrows take, so a line ends where the arrow would have been.
fn record_utc_ms(record: &ChartTradeRecord, axis: &moon_core::db::ReportAxis) -> (i64, i64) {
    axis.stamp_pair_to_utc_ms(record.buy_stamp(), record.close_stamp(), record.core_uid)
}

#[cfg(test)]
mod tests;
