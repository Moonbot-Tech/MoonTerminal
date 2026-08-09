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
    /// Projected profit of the NEWEST closed trade in the period, in the same unit as `profit`.
    ///
    /// `None` when that trade carries no projected profit at all — the same NULL the additive sum
    /// wraps in `COALESCE`. It stays an option rather than becoming zero because a displayed
    /// `(+0)` is a claim about a trade, and a trade whose profit is unknown made no such claim.
    pub last_profit: Option<f64>,
    /// Close date of the newest trade, or zero when the period holds none for this core.
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
                // Filled by the second pass below, which is the only place that can spend a
                // min/max on the close date without taking it away from the name above.
                last_profit: None,
                last_close: 0,
            })
        })
        .map_err(|error| read_fail_on(conn, CTX, error))?;
    let mut cores = Vec::new();
    for row in rows {
        cores.push(row.map_err(|error| read_fail_on(conn, CTX, error))?);
    }
    let latest = latest_trade_on(conn, &source, &q)?;
    for core in &mut cores {
        if let Some((profit, close)) = latest.get(&core.core_uid) {
            core.last_profit = *profit;
            core.last_close = *close;
        }
    }
    Ok(scoped(decision, ProfitMonitorSummary { cores }))
}

/// Read the newest closed trade of every core over the same projected source.
///
/// A SECOND pass on purpose, and a cheap one: the aggregate above already spends its single
/// MIN/MAX on the latest NONBLANK `core_name`, and SQLite only lets bare columns follow ONE
/// min/max. Here the aggregate is spent on the close date instead, so the bare `pnl` is the newest
/// trade's own profit. Both passes are plain `GROUP BY` scans and neither has to sort.
///
/// The alternative — ranking rows with `ROW_NUMBER() OVER (...)` so both answers come from one
/// query with no bare column at all — was built and MEASURED against this shape on a
/// 50 000-row / 40-core fixture: it forces SQLite to sort the whole filtered period, and the read
/// went from roughly 30 ms to 72 ms. The monitor runs this on every report generation, so the
/// ranked form was not worth its only advantage, which is the tie below.
///
/// The tie: two trades of one core closing in the SAME Unix second leave SQLite free to take the
/// bare `pnl` from either. It is unspecified rather than random — identical data and plan give the
/// same row — so the displayed amount is stable in practice; `a_same_second_tie_still_has_one_
/// stable_last_trade` is the guard that says so out loud, and it is the assumption to revisit if a
/// future SQLite ever makes that choice vary.
///
/// Args:
///     conn: Existing report reader or pinned snapshot.
///     source: Projected unified source already built for the aggregate pass.
///     q: Query supplying the same period bounds.
///
/// Returns:
///     Newest `(projected profit, close date)` keyed by core, or a classified read failure. The
///     profit stays optional: a source row can carry a NULL projected profit — the very NULL the
///     aggregate's `COALESCE` exists for — and printing that as `(+0)` would claim a trade made
///     nothing when what it made is unknown.
fn latest_trade_on(
    conn: &Connection,
    source: &str,
    q: &Query,
) -> ReadResult<std::collections::HashMap<u64, (Option<f64>, i64)>> {
    const CTX: &str = "analytics: profit monitor last trade";

    let sql = format!(
        "SELECT o.core_uid, o.pnl, MAX(o.closedate)
         FROM {source}
         GROUP BY o.core_uid"
    );
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| read_fail_on(conn, CTX, error))?;
    let rows = statement
        .query_map(rusqlite::params![q.from, q.to], |row| {
            Ok((
                row.get::<_, Option<i64>>(0)?.unwrap_or(0).max(0) as u64,
                (
                    row.get::<_, Option<f64>>(1)?,
                    row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                ),
            ))
        })
        .map_err(|error| read_fail_on(conn, CTX, error))?;
    let mut latest = std::collections::HashMap::new();
    for row in rows {
        let (core, value) = row.map_err(|error| read_fail_on(conn, CTX, error))?;
        latest.insert(core, value);
    }
    Ok(latest)
}
