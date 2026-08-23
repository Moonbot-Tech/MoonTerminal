//! Opening the trade-detail window from a Report row.
//!
//! The row the user clicked carries only DISPLAY cells, so the typed trade has to be read back
//! from the durable replica. That read is the SAME one the main chart's trade markers already
//! make — same reader, same snapshot, same projection — rather than a second query with its own
//! opinion about what a trade is.

use chrono::TimeZone;
use gpui::*;
use moon_core::db::{self, ChartTradeRecord, ReportFilter};
use rust_i18n::t;

use super::{ReportPanel, columns, selection};

/// Bound on the durable read, matching the chart's own trade-history cap.
const HISTORY_LIMIT: usize = 1_000;

/// Everything the opener needs from the clicked row, resolved on the UI thread.
///
/// RESOLVED EAGERLY and then carried, never re-resolved from a row index later. A Report refresh
/// runs in the background and republishes the row ordering when it lands, so a retained index is
/// a promise about a table that may no longer exist — a context menu left open across a refresh
/// would then act on whichever trade now occupies that position. The adjacent trade-log action
/// captures its request the same way and for the same reason.
#[derive(Clone)]
pub(super) struct RowTarget {
    core: u64,
    coin: String,
    record_id: i64,
    market: String,
    filter: ReportFilter,
}

/// Render one Unix second in the Report's own display zone.
///
/// The zone is the panel's, not a second clock of the window's own: the times beside the chart
/// must read exactly as the row the user clicked.
///
/// Args:
///     seconds: Unix seconds.
///     zone: The Report's display zone.
///
/// Returns:
///     `YYYY-MM-DD HH:MM:SS`, or a dash for an unusable stamp.
fn stamp(seconds: i64, zone: chrono_tz::Tz) -> String {
    match zone.timestamp_opt(seconds, 0).single() {
        Some(moment) => moment.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => "-".to_string(),
    }
}

impl ReportPanel {
    /// Open the dedicated window for the trade on one row, if that row can be resolved.
    ///
    /// Silent when the row cannot be resolved — a double-click has nowhere to put a reason, which
    /// is precisely why the row MENU carries the same action with a disabled arm that states one.
    ///
    /// Args:
    ///     row: Visible row index.
    ///     cx: Panel context.
    pub(super) fn open_trade_detail(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(target) = self.trade_detail_target(row, cx) else {
            return;
        };
        self.open_trade_detail_target(target, cx);
    }

    /// Open the window for a target resolved EARLIER.
    ///
    /// The row-menu path resolves at menu-build time and calls this, so the action cannot drift
    /// onto a different trade if the table is republished while the menu is open.
    ///
    /// Args:
    ///     target: The already-resolved row.
    ///     cx: Panel context.
    pub(super) fn open_trade_detail_target(&mut self, target: RowTarget, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let zone = self.display_zone;
        let RowTarget {
            core,
            coin,
            record_id,
            market,
            filter,
        } = target;
        // The durable read is SQLite and belongs off the UI thread, exactly as the chart's own
        // trade-history read is.
        cx.spawn(async move |_this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let found = executor
                .spawn(async move { load_trade(core, coin, record_id, filter) })
                .await;
            cx.update(|cx| {
                let Some(record) = found else {
                    return;
                };
                let stamps = (stamp(record.buy_date, zone), stamp(record.close_date, zone));
                crate::trade_window::open_trade_window(&backend, record, core, market, stamps, cx);
            });
        })
        .detach();
    }

    /// Resolve one row into everything the window needs, or nothing.
    ///
    /// A `None` here is also what the row menu renders its disabled arm from: the market resolves
    /// against the core's LIVE catalog, so an offline core stops here — the same boundary the
    /// existing coin cell already stops at.
    ///
    /// Args:
    ///     row: Visible row index.
    ///     cx: Panel context.
    ///
    /// Returns:
    ///     The resolved target, or `None`.
    pub(super) fn trade_detail_target(&self, row: usize, cx: &App) -> Option<RowTarget> {
        let data = self.data.data()?;
        let core = data.core_uids.get(row).copied()?;
        let record_id = match data.row_keys.get(row)?.as_ref()? {
            selection::ReportRowKey::Replicated { rec_id, .. } => *rec_id,
            selection::ReportRowKey::Legacy { db_id, .. } => *db_id,
        };
        let values = data.rows.get(row)?;
        let coin = self
            .cols
            .iter()
            .position(|col| col == "coin")
            .and_then(|ix| values.get(ix))
            .map(columns::value_to_string)
            .filter(|coin| !coin.is_empty())?;
        // Reused rather than copied: this is the coin-to-market rule including the folded-token
        // catalog lookup, and a second spelling of it would open charts on markets that exist
        // nowhere.
        let market = columns::resolve_market(self.backend.read(cx), core, &coin)?;
        Some(RowTarget {
            core,
            coin,
            record_id,
            market,
            filter: (*data.filter).clone(),
        })
    }
}

/// Read one trade back from the durable replica.
///
/// Scoped to the row's own coin so the bounded read is spent where the target actually is; a
/// whole-filter read would be both slower and likelier to push the target past the cap.
///
/// Args:
///     core: Core that recorded the row.
///     coin: The row's coin token, as stored.
///     record_id: Durable record id of the clicked trade.
///     filter: The published filter that produced the row.
///
/// Returns:
///     The typed trade, or `None` when the replica cannot answer or the row fell past the cap.
fn load_trade(
    core: u64,
    coin: String,
    record_id: i64,
    filter: ReportFilter,
) -> Option<ChartTradeRecord> {
    let conn = db::open_reader().ok()?;
    let snapshot = db::read_snapshot(&conn).ok()?;
    let history = db::query_chart_trade_history(
        &snapshot,
        core,
        std::slice::from_ref(&coin),
        Some(&filter),
        HISTORY_LIMIT,
    )
    .ok()?;
    history
        .records
        .into_iter()
        .find(|record| record.record_id == record_id)
}

/// Label of the row-menu entry that opens this window.
///
/// Returns:
///     Localized menu label.
pub(super) fn menu_label() -> String {
    t!("trade_window.open").to_string()
}
