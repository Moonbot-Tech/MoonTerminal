//! Compact per-core aggregates for the desktop Profit Monitor.

#[cfg(test)]
mod tests;

use rusqlite::Connection;

mod latest_name;
mod latest_trade;

use super::{
    Query, ScopeDecision, min_closedate, read_core_scope, scope_decision_with_quote_reader_on,
    scoped, unified_from_mode,
};
use crate::db::read_fail::read_fail_on;
use crate::db::{ProfitScope, QuoteCurrency, ReadFail, ReadResult};

/// One core's additive metrics over the selected monitor period.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfitMonitorCore {
    /// Stable report-side core identifier.
    pub core_uid: u64,
    /// Name carried by the latest trade in the selected period.
    pub report_name: String,
    /// Profit projected into the enclosing [`ProfitScope`] unit.
    pub profit: f64,
    /// Number of closed trades.
    pub trades: i64,
    /// Number of trades with positive projected profit.
    pub wins: i64,
    /// Sum of positive order spends, retained so grouped rows remain exact.
    pub positive_spent: f64,
    /// Number of trades contributing to `positive_spent`.
    pub positive_orders: i64,
    /// Projected profit of the NEWEST closed trade in the period, in the same unit as `profit`.
    ///
    /// `None` when that trade carries no projected profit at all — the same NULL the additive sum
    /// wraps in `COALESCE`. It stays an option rather than becoming zero because a displayed
    /// `(+0)` is a claim about a trade, and a trade whose profit is unknown made no such claim.
    pub last_profit: Option<f64>,
    /// Close date of the newest trade, or zero when absent or not stored as integer seconds.
    ///
    /// Grouping merges cores by taking the largest of these, and the monitor's arrival highlight
    /// fires on this value CHANGING — an authoritative timestamp rather than a guess from a count.
    pub last_close: i64,
}

impl ProfitMonitorCore {
    /// Return the percentage of profitable trades.
    ///
    /// Returns:
    ///     A percentage in `0..=100`, or zero when the row has no trades.
    pub fn win_rate(&self) -> f64 {
        if self.trades == 0 {
            0.0
        } else {
            self.wins as f64 * 100.0 / self.trades as f64
        }
    }

    /// Return the average positive order spend.
    ///
    /// Returns:
    ///     The exact additive sum divided by its contributing count, or zero when no positive
    ///     spend exists.
    pub fn average_order(&self) -> f64 {
        if self.positive_orders == 0 {
            0.0
        } else {
            self.positive_spent / self.positive_orders as f64
        }
    }
}

/// Compact monitor payload produced by one grouped database scan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfitMonitorSummary {
    /// One additive row per report core.
    pub cores: Vec<ProfitMonitorCore>,
}

/// One native currency partition; unknown denominations carry counts but no money.
#[derive(Clone, Debug)]
pub struct ProfitMonitorCurrency {
    /// Trusted denomination, absent for unknown report currency values.
    pub currency: Option<QuoteCurrency>,
    /// Per-core metrics from this partition only.
    pub data: ProfitMonitorSummary,
}

/// Scalar coverage and native fallback rows read from the same committed snapshot.
pub struct ProfitMonitorSnapshot {
    /// Existing comparability contract, including exact split totals.
    pub scope: ProfitScope<ProfitMonitorSummary>,
    /// Populated only when the whole scope cannot publish comparable money.
    pub currencies: Vec<ProfitMonitorCurrency>,
}

/// Read monitor rows without withholding native partitions during conversion outages.
///
/// Args:
///     q: Period, filters, and configured core scope.
///
/// Returns:
///     Comparable or split native monitor rows from one pinned report snapshot.
pub fn profit_monitor_snapshot(q: &Query) -> ReadResult<ProfitMonitorSnapshot> {
    profit_monitor_snapshot_with_replica_scope(q, None)
}

/// Read monitor rows with optional first-result discovery of replica-only cores.
///
/// Args:
///     q: Period, filters, and configured fallback core scope.
///     replica_hidden: Hidden configured ids when the active preset narrows an implicit All;
///         `None` leaves `q.cores` untouched.
///
/// Returns:
///     Comparable or split native monitor rows from one pinned report snapshot, with replica
///     discovery applied before aggregation.
pub fn profit_monitor_snapshot_with_replica_scope(
    q: &Query,
    replica_hidden: Option<&[u64]>,
) -> ReadResult<ProfitMonitorSnapshot> {
    let conn = super::super::open_reader()?;
    let _rates = super::super::valuation::pin_current_rates();
    let snapshot = super::super::read_snapshot(&conn)?;
    snapshot_on(&snapshot, q, replica_hidden)
}

/// Keep every currency partition under the same period, filters, and SQLite snapshot.
///
/// Args:
///     conn: Existing report reader or pinned snapshot.
///     q: Period, filters, and configured fallback core scope.
///     replica_hidden: Hidden configured ids for first-read replica discovery.
///
/// Returns:
///     Comparable or split native monitor rows with identical scope across every partition.
fn snapshot_on(
    conn: &Connection,
    q: &Query,
    replica_hidden: Option<&[u64]>,
) -> ReadResult<ProfitMonitorSnapshot> {
    let scope = read_core_scope(conn, q, false, replica_hidden);
    let mut q = scope.query(q).into_owned();
    if q.from < 0 {
        q.from = min_closedate(conn, &q.resolved_axis(conn)?)?;
    }
    let scope = profit_monitor_on(conn, &q)?;
    let mut currencies = Vec::new();
    if let ProfitScope::Split(totals) = &scope {
        let source = unified_from_mode(conn, &q, super::query::ProjectionMode::Native)?
            .ok_or(ReadFail::NotReady)?;
        for currency in totals
            .totals
            .iter()
            .map(|total| Some(total.currency))
            .chain((totals.unknown_orders > 0).then_some(None))
        {
            let predicate = match currency {
                Some(currency) => format!("typeof(basecurrency)='integer' AND basecurrency={}", currency.ordinal()),
                None => "basecurrency IS NULL OR typeof(basecurrency)<>'integer' OR basecurrency NOT BETWEEN 0 AND 20".to_string(),
            };
            let partition = format!("(SELECT * FROM {source} WHERE {predicate}) o");
            let mut data = aggregate_on(conn, &q, &partition)?;
            if currency.is_none() {
                for core in &mut data.cores {
                    core.profit = 0.0;
                    core.last_profit = None;
                    core.positive_spent = 0.0;
                    core.positive_orders = 0;
                }
            }
            currencies.push(ProfitMonitorCurrency { currency, data });
        }
    }
    Ok(ProfitMonitorSnapshot { scope, currencies })
}

/// Read the compact Profit Monitor payload from one committed report snapshot.
///
/// Args:
///     q: Period, core filters, trade filters, metric, and valuation mode.
///
/// Returns:
///     Comparable or empty per-core rows, split quote totals, or a classified read failure.
pub fn profit_monitor(q: &Query) -> ReadResult<ProfitScope<ProfitMonitorSummary>> {
    let conn = super::super::open_reader()?;
    let _rates = super::super::valuation::pin_current_rates();
    let snapshot = super::super::read_snapshot(&conn)?;
    profit_monitor_on(&snapshot, q)
}

/// Aggregate monitor rows on an existing reader or pinned snapshot.
///
/// Args:
///     conn: Existing report reader or pinned snapshot.
///     q: Active Analytics filters and period.
///
/// Returns:
///     Comparable or empty per-core rows, split quote totals, or a classified read failure.
pub(super) fn profit_monitor_on(
    conn: &Connection,
    q: &Query,
) -> ReadResult<ProfitScope<ProfitMonitorSummary>> {
    let mut q = q.clone();
    if q.from < 0 {
        q.from = min_closedate(conn, &q.resolved_axis(conn)?)?;
    }
    let decision = match scope_decision_with_quote_reader_on(
        conn,
        &q,
        super::query::quote_breakdown_unfenced_on,
    )? {
        ScopeDecision::Split(totals) => return Ok(ProfitScope::Split(totals)),
        decision => decision,
    };
    let projection = decision
        .projection()
        .expect("split decisions return before monitor source construction");
    let Some(source) = unified_from_mode(conn, &q, projection)? else {
        return Err(ReadFail::NotReady);
    };
    Ok(scoped(decision, aggregate_on(conn, &q, &source)?))
}

/// Aggregate a prefiltered source whose money shares one explicitly established unit.
fn aggregate_on(conn: &Connection, q: &Query, source: &str) -> ReadResult<ProfitMonitorSummary> {
    const CTX: &str = "analytics: profit monitor";
    conn.create_aggregate_function(
        "mt_latest_report_name",
        2,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        latest_name::LatestNameAggregate,
    )
    .map_err(|error| read_fail_on(conn, CTX, error))?;
    conn.create_aggregate_function(
        "mt_latest_report_trade",
        4,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        latest_trade::LatestTradeAggregate,
    )
    .map_err(|error| read_fail_on(conn, CTX, error))?;
    // Both latest-row choices are explicit aggregates in the same grouped pass: no bare column
    // depends on SQLite's arbitrary choice among equal MAX dates.
    let sql = format!(
        "SELECT o.core_uid,
                mt_latest_report_name(CASE WHEN TRIM(COALESCE(o.core_name, '')) <> '' THEN o.closedate END, o.core_name),
                COALESCE(SUM(o.pnl), 0.0),
                COUNT(*),
                COUNT(CASE WHEN o.pnl > 0.0 THEN 1 END),
                COALESCE(SUM(CASE WHEN o.spentbtc > 0.0 THEN o.spentbtc ELSE 0.0 END), 0.0),
                COUNT(CASE WHEN o.spentbtc > 0.0 THEN 1 END),
                mt_latest_report_trade(
                    o.closedate,
                    CASE WHEN o.closedatems > 0 THEN o.closedatems ELSE o.closedate * 1000 END,
                    COALESCE(o.newrecid, o.db_id, o.reportuid),
                    o.pnl
                ),
                MAX(o.closedate)
         FROM {source}
         GROUP BY o.core_uid
         ORDER BY o.core_uid"
    );
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| read_fail_on(conn, CTX, error))?;
    let rows = statement
        .query_map(rusqlite::params![q.from, q.to], |row| {
            Ok(ProfitMonitorCore {
                core_uid: row.get::<_, Option<i64>>(0)?.unwrap_or(0).max(0) as u64,
                report_name: row
                    .get::<_, Option<String>>(1)?
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                profit: row.get::<_, f64>(2)?,
                trades: row.get::<_, i64>(3)?,
                wins: row.get::<_, i64>(4)?,
                positive_spent: row.get::<_, f64>(5)?,
                positive_orders: row.get::<_, i64>(6)?,
                last_profit: row.get(7)?,
                // Only stored integer seconds can drive the arrival timestamp. Other SQLite
                // storage classes still select the last profit but carry no usable timestamp.
                last_close: match row.get_ref(8)? {
                    rusqlite::types::ValueRef::Integer(close) => close,
                    _ => 0,
                },
            })
        })
        .map_err(|error| read_fail_on(conn, CTX, error))?;
    let mut cores = Vec::new();
    for row in rows {
        cores.push(row.map_err(|error| read_fail_on(conn, CTX, error))?);
    }
    // Preserve the existing normalization of malformed negative/NULL core ids, including
    // the last grouped row owning their shared latest-trade entry.
    let latest = cores
        .iter()
        .map(|core| (core.core_uid, (core.last_profit, core.last_close)))
        .collect::<std::collections::HashMap<_, _>>();
    for core in &mut cores {
        if let Some((profit, close)) = latest.get(&core.core_uid) {
            core.last_profit = *profit;
            core.last_close = *close;
        }
    }
    Ok(ProfitMonitorSummary { cores })
}
