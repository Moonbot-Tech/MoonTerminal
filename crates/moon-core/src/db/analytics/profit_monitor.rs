//! Compact per-core aggregates for the desktop Profit Monitor.

#[cfg(test)]
mod tests;

use rusqlite::Connection;

use super::{min_closedate, scope_decision_on, scoped, unified_from_mode, Query, ScopeDecision};
use crate::db::read_fail::read_fail_on;
use crate::db::{ProfitScope, ReadFail, ReadResult};

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
    const CTX: &str = "analytics: profit monitor";

    let mut q = q.clone();
    if q.from < 0 {
        q.from = min_closedate(conn)?;
    }
    let decision = match scope_decision_on(conn, &q)? {
        ScopeDecision::Split(totals) => return Ok(ProfitScope::Split(totals)),
        decision => decision,
    };
    let projection = decision
        .projection()
        .expect("split decisions return before monitor source construction");
    let Some(source) = unified_from_mode(conn, &q, projection)? else {
        return Err(ReadFail::NotReady);
    };
    // Keep exactly one MIN/MAX aggregate: SQLite then guarantees that the bare `core_name` comes
    // from the row supplying this latest nonblank close date. SUM and COUNT do not weaken that
    // rule, while a second MIN/MAX would silently make the chosen name arbitrary.
    let sql = format!(
        "SELECT o.core_uid,
                o.core_name,
                COALESCE(SUM(o.pnl), 0.0),
                COUNT(*),
                COUNT(CASE WHEN o.pnl > 0.0 THEN 1 END),
                COALESCE(SUM(CASE WHEN o.spentbtc > 0.0 THEN o.spentbtc ELSE 0.0 END), 0.0),
                COUNT(CASE WHEN o.spentbtc > 0.0 THEN 1 END),
                MAX(CASE WHEN TRIM(COALESCE(o.core_name, '')) <> '' THEN o.closedate END)
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
            })
        })
        .map_err(|error| read_fail_on(conn, CTX, error))?;
    let mut cores = Vec::new();
    for row in rows {
        cores.push(row.map_err(|error| read_fail_on(conn, CTX, error))?);
    }
    Ok(scoped(decision, ProfitMonitorSummary { cores }))
}
