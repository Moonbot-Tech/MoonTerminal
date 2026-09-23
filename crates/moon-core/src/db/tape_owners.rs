//! The report rows that can claim a stretch of the trade tape (`trades.sqlite`): what the
//! Storage tab's cleanup reads to decide which prints still have a trade behind them.
//!
//! One narrow scan of the replica, bounded in time by what the tape file actually holds — the
//! file spans days where the replica spans years, and a row that closed before the file's first
//! print or opened after its last cannot claim any of it. Every closed row inside the bound is
//! read, service rows and manual exits included — the scan does not decide which rows count;
//! which rows are the tuner's is the row's own to say ([`TapeOwner::is_tunable`]), by the same
//! rule the axis applies (`db::tuner::ticks::scope`), and the cleanup lets those claim.

use super::report_axis::ReportStamp;
use super::tuner::ticks::{is_service_row, is_tunable};
use super::{ReadResult, rep};

/// One closed report row, narrowed to what the tape cleanup resolves it with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TapeOwner {
    pub core_uid: u64,
    /// The coin as the report spells it (`BEN`, or a full market on old rows).
    pub coin: String,
    /// Entry stamp, core-local — lift it through `ReportAxis` like the trade window does.
    pub buy: ReportStamp,
    /// Exit stamp, same caveat.
    pub close: ReportStamp,
    /// The entry order's creation (`buysetdatems`), core-local milliseconds like the entry's —
    /// the tuner fetches a trade's tape from there (`tuner::ticks::model_window_at`), so the
    /// cleanup must claim from there too. `None` on rows and replicas that do not carry it.
    pub buy_set_ms: Option<i64>,
    pub strategy_id: i64,
    /// `sellreason` as the core wrote it.
    pub sell_reason: String,
    /// The strategy's kind from `strategies.sqlite`; empty when the database does not know it.
    pub kind: String,
}

impl TapeOwner {
    /// Whether the tuner can be run on this row: not a service row, a tunable kind, an exit
    /// the strategy made itself — the axis' own filter, so "keep the tuner's tape" and "what the
    /// tuner reads" are one rule.
    pub fn is_tunable(&self) -> bool {
        !is_service_row(self.strategy_id, &self.sell_reason)
            && is_tunable(&self.kind, &self.sell_reason)
    }
}

/// Every closed row that closed at or after `closed_from_s` and opened at or before
/// `opened_to_s` (core-local seconds, the replica's own columns), with its strategy kind
/// resolved.
///
/// Args:
///     closed_from_s: Earliest exit to include, seconds.
///     opened_to_s: Latest entry to include, seconds.
///
/// Returns:
///     The rows, in no particular order; `NotReady` when the replica is absent.
pub fn read_tape_owners(closed_from_s: i64, opened_to_s: i64) -> ReadResult<Vec<TapeOwner>> {
    let conn = super::open_reader()?;
    let mut owners = read_rows(&conn, closed_from_s, opened_to_s)?;
    // The kind lives in strategies.sqlite; one lookup per distinct strategy, like the axis.
    let mut pairs: Vec<(i64, u64)> = owners
        .iter()
        .filter(|o| o.strategy_id != 0)
        .map(|o| (o.strategy_id, o.core_uid))
        .collect();
    pairs.sort_unstable();
    pairs.dedup();
    let kinds = super::tuner::strategy_kinds(&pairs);
    for owner in &mut owners {
        if let Some(kind) = kinds.get(&(owner.strategy_id, owner.core_uid)) {
            owner.kind = kind.clone();
        }
    }
    Ok(owners)
}

/// The scan itself, on an open reader; kinds left empty.
fn read_rows(
    conn: &rusqlite::Connection,
    closed_from_s: i64,
    opened_to_s: i64,
) -> ReadResult<Vec<TapeOwner>> {
    const CTX: &str = "reports: tape owners";
    let cols = rep::table_cols_res(conn)?;
    // A replica that predates the millisecond columns still names every trade: the stamps
    // then resolve to the seconds columns, exactly as the trade window reads such a row.
    let column = |name: &str| {
        if cols.contains(name) {
            name.to_string()
        } else {
            "NULL".to_string()
        }
    };
    let sql = format!(
        "SELECT core_uid, coin, buydate, closedate, {buy_ms}, {close_ms}, strategyid, sellreason,
                {buy_set_ms}
         FROM {table}
         WHERE closedate > 0 AND closedate >= ?1 AND buydate <= ?2",
        buy_ms = column("buydatems"),
        close_ms = column("closedatems"),
        buy_set_ms = column("buysetdatems"),
        table = rep::TABLE,
    );
    let fail = |e: rusqlite::Error| super::read_fail::read_fail(CTX, e);
    let mut stmt = conn.prepare(&sql).map_err(fail)?;
    let rows = stmt
        .query_map(rusqlite::params![closed_from_s, opened_to_s], |r| {
            let buy_s: i64 = r.get(2)?;
            let close_s: i64 = r.get(3)?;
            let buy_ms: Option<i64> = r.get(4)?;
            let close_ms: Option<i64> = r.get(5)?;
            Ok(TapeOwner {
                core_uid: r.get::<_, i64>(0)? as u64,
                coin: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                buy: ReportStamp::resolve(buy_s, buy_ms),
                close: ReportStamp::resolve(close_s, close_ms),
                strategy_id: r.get::<_, Option<i64>>(6)?.unwrap_or(0),
                sell_reason: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                buy_set_ms: r.get::<_, Option<i64>>(8)?.filter(|&set| set > 0),
                kind: String::new(),
            })
        })
        .map_err(fail)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(fail);
    rows
}

#[cfg(test)]
mod tests;
