//! Opening the trade-detail window from a Report row.
//!
//! The row the user clicked carries only DISPLAY cells, so the typed trade has to be read back
//! from the durable replica. That read is the SAME one the main chart's trade markers already
//! make — same reader, same snapshot, same projection — rather than a second query with its own
//! opinion about what a trade is.

use chrono::TimeZone;
use gpui::*;
use moon_core::db::{self, ChartTradeRecord, ReportFilter, TradeMeta};
use rust_i18n::t;

use super::{ReportPanel, columns, selection};

/// SQLite's largest positive limit, reserving the history reader's truncation probe row.
/// A trade window must retain the whole period, including a clicked row older than 1,000 trades.
const HISTORY_LIMIT: usize = (i64::MAX - 1) as usize;

#[cfg(test)]
mod tests;

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

/// Render one replicated stamp on the Report's own time axis.
///
/// The zone is the panel's, not a second clock of the window's own: the times beside the chart
/// must read exactly as the row the user clicked. The lift from core-local to true UTC happens
/// inside: `buy_date`/`close_date` (and their millisecond siblings) carry the CORE's wall clock,
/// so the user's display zone must not be applied on top of an already-zoned value.
///
/// Args:
///     axis: Report axis that both lifts the stamp and supplies the display zone.
///     core: Stable uid of the core that produced the row.
///     stamp: The typed core-local stamp for this end.
///
/// Returns:
///     `YYYY-MM-DD HH:MM:SS` or `YYYY-MM-DD HH:MM:SS.mmm`, or a dash for an unusable stamp.
fn stamp(axis: &moon_core::db::ReportAxis, core: u64, stamp: moon_core::db::ReportStamp) -> String {
    let Some(moment) = axis
        .zone()
        .timestamp_millis_opt(axis.stamp_to_utc_ms(stamp, core))
        .single()
    else {
        return "-".to_string();
    };
    match stamp {
        moon_core::db::ReportStamp::Seconds(_) => moment.format("%Y-%m-%d %H:%M:%S"),
        moon_core::db::ReportStamp::Millis(_) => moment.format("%Y-%m-%d %H:%M:%S%.3f"),
    }
    .to_string()
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
        // These stamps render replicated columns, so they follow the report axis rather than the
        // header clock -- the same split the Report grid makes for the very same two values. That
        // means BOTH halves of the axis: the offset that lifts a core-local second to true UTC,
        // and only then the zone it renders in. Taking the zone alone is what let this window
        // disagree with the very row that opened it.
        let axis = self.report_axis();
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
                let Some((record, meta, history)) = found else {
                    return;
                };
                let stamps = (
                    stamp(&axis, core, record.buy_stamp()),
                    stamp(&axis, core, record.close_stamp()),
                );
                crate::trade_window::open_trade_window(
                    &backend, record, meta, history, market, stamps, cx,
                );
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

/// Read the focused trade, its metadata, and every neighbour on one durable snapshot.
///
/// The history query preserves the published non-period predicates and exact core/coin scope.
/// Its usual exit-only period is applied here to either endpoint instead. Metadata failure still
/// degrades to empty captions, as before; history failure prevents opening a misleading window.
///
/// Args:
///     core: Core that recorded the row.
///     coin: Exact stored coin token.
///     record_id: Clicked record identity.
///     filter: Published Report filter, including its time axis.
///
/// Returns:
///     Focus, metadata and period history, or `None` when the replica cannot resolve the focus.
fn load_trade(
    core: u64,
    coin: String,
    record_id: i64,
    filter: ReportFilter,
) -> Option<(ChartTradeRecord, TradeMeta, Vec<ChartTradeRecord>)> {
    let conn = db::open_reader().ok()?;
    let snapshot = db::read_snapshot(&conn).ok()?;
    let history = period_history(&snapshot, core, &coin, &filter).ok()?;
    let record = history
        .iter()
        .find(|record| record.record_id == record_id)?
        .clone();
    let meta = db::query_trade_meta(&snapshot, &record)
        .ok()
        .flatten()
        .unwrap_or_default();
    Some((record, meta, history))
}

/// Read all matching closed trades and retain those with either endpoint in the Report period.
///
/// Uses the same current per-core offset and inclusive seconds bounds as the Report SQL. Keeping
/// selection outside the shared reader leaves the Main chart's exit-only history unchanged.
fn period_history(
    snapshot: &rusqlite::Connection,
    core: u64,
    coin: &str,
    filter: &ReportFilter,
) -> db::ReadResult<Vec<ChartTradeRecord>> {
    let mut scope = filter.clone();
    scope.date_from = None;
    scope.date_to = None;
    let history = db::query_chart_trade_history(
        snapshot,
        core,
        &[coin.to_owned()],
        Some(&scope),
        HISTORY_LIMIT,
    )?;
    let now = moon_core::util::now_unix_ms_i64().div_euclid(1_000);
    let offset = filter.axis.offset_secs(core, now).unwrap_or(0);
    let from = filter
        .date_from
        .map(|bound| db::ReportAxis::shift_bound(bound, offset));
    let to = filter
        .date_to
        .map(|bound| db::ReportAxis::shift_bound(bound, offset));
    let inside = |stamp| from.is_none_or(|from| stamp >= from) && to.is_none_or(|to| stamp <= to);
    Ok(history
        .records
        .into_iter()
        .filter(|record| inside(record.buy_date) || inside(record.close_date))
        .collect())
}

/// Label of the row-menu entry that opens this window.
///
/// Returns:
///     Localized menu label.
pub(super) fn menu_label() -> String {
    t!("trade_window.open").to_string()
}
