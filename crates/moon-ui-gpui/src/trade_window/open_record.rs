//! Opening the trade-detail window from a REPLICA RECORD — the Report row, the tuner's deal
//! row, any list that names a trade by its core, coin and one of the replica's two identities
//! ([`RecordKey`]).
//!
//! The row the user clicked carries only DISPLAY cells, so the typed trade has to be read back
//! from the durable replica. That read is the SAME one the main chart's trade markers already
//! make — same reader, same snapshot, same projection — rather than a second query with its own
//! opinion about what a trade is. One opener for every list, so a second list cannot grow a
//! second opinion either.

use chrono::TimeZone;
use gpui::*;
use moon_core::db::{self, ChartTradeRecord, ReportFilter, TradeMeta};

use crate::Backend;

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
/// would then act on whichever trade now occupies that position.
#[derive(Clone)]
pub(crate) struct RecordTarget {
    /// Stable uid of the core that produced the row.
    pub(crate) core: u64,
    /// The coin as the replica spells it.
    pub(crate) coin: String,
    /// Which row of the replica: see [`RecordKey`].
    pub(crate) record: RecordKey,
    /// Exchange-native market the window's chart and tape are keyed by.
    pub(crate) market: String,
    /// The scope the window's neighbours are read under: its non-period predicates as they
    /// are, its period applied to either endpoint ([`period_history`]).
    pub(crate) filter: ReportFilter,
}

/// The identity a caller names a replica row by. The two are DIFFERENT counters and never stand
/// in for each other ([`ChartTradeRecord::report_uid`] says so): the Report table addresses its
/// rows by the replica's own `newrecid`/row id, the tuner's deal table by the core's `ReportUID`,
/// the key the order traces and the tape coverage are filed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordKey {
    /// [`ChartTradeRecord::record_id`].
    RecordId(i64),
    /// [`ChartTradeRecord::report_uid`]; a row replicated without one can never match.
    ReportUid(i64),
}

impl RecordKey {
    /// Whether `record` is the row this key names.
    fn matches(self, record: &ChartTradeRecord) -> bool {
        match self {
            Self::RecordId(id) => record.record_id == id,
            Self::ReportUid(uid) => record.report_uid == Some(uid),
        }
    }
}

/// Render one replicated stamp on the caller's time axis.
///
/// The zone is the caller's, not a second clock of the window's own: the times beside the chart
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
fn stamp(axis: &db::ReportAxis, core: u64, stamp: db::ReportStamp) -> String {
    let Some(moment) = axis
        .zone()
        .timestamp_millis_opt(axis.stamp_to_utc_ms(stamp, core))
        .single()
    else {
        return "-".to_string();
    };
    match stamp {
        db::ReportStamp::Seconds(_) => moment.format("%Y-%m-%d %H:%M:%S"),
        db::ReportStamp::Millis(_) => moment.format("%Y-%m-%d %H:%M:%S%.3f"),
    }
    .to_string()
}

/// Open the window for a target resolved EARLIER, or focus the one already open on it.
///
/// Silent when the replica cannot resolve the record — a double-click has nowhere to put a
/// reason, which is why a row MENU beside it carries the same action with a disabled arm that
/// states one.
///
/// Args:
///     backend: The application backend the window binds to.
///     axis: Time axis the window's captions render on — BOTH halves: the offset that lifts a
///         core-local second to true UTC, and only then the zone it renders in. Taking the zone
///         alone is what once let this window disagree with the very row that opened it.
///     target: The already-resolved row.
///     cx: Application context.
pub(crate) fn open_trade_record(
    backend: &Entity<Backend>,
    axis: db::ReportAxis,
    target: RecordTarget,
    cx: &mut App,
) {
    let backend = backend.clone();
    let RecordTarget {
        core,
        coin,
        record,
        market,
        filter,
    } = target;
    // The durable read is SQLite and belongs off the UI thread, exactly as the chart's own
    // trade-history read is.
    cx.spawn(async move |cx| {
        let executor = cx.update(|cx| cx.background_executor().clone());
        let found = executor
            .spawn(async move { load_trade(core, coin, record, filter) })
            .await;
        cx.update(|cx| {
            let Some((record, meta, history)) = found else {
                return;
            };
            let stamps = (
                stamp(&axis, core, record.buy_stamp()),
                stamp(&axis, core, record.close_stamp()),
            );
            super::open_trade_window(&backend, record, meta, history, market, stamps, cx);
        });
    })
    .detach();
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
///     record: Clicked record identity.
///     filter: Published filter, including its time axis.
///
/// Returns:
///     Focus, metadata and period history, or `None` when the replica cannot resolve the focus.
fn load_trade(
    core: u64,
    coin: String,
    record: RecordKey,
    filter: ReportFilter,
) -> Option<(ChartTradeRecord, TradeMeta, Vec<ChartTradeRecord>)> {
    let conn = db::open_reader_with(db::CHART_TRADE_HISTORY_ATTACH).ok()?;
    let snapshot = db::read_snapshot(&conn).ok()?;
    let history = period_history(&snapshot, core, &coin, &filter).ok()?;
    let record = history.iter().find(|row| record.matches(row))?.clone();
    let meta = db::query_trade_meta(&snapshot, &record)
        .ok()
        .flatten()
        .unwrap_or_default();
    Some((record, meta, history))
}

/// Read all matching closed trades and retain those with either endpoint in the filter's period.
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
