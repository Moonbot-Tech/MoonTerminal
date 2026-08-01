//! Calendar heatmap cells and hour-of-day profiles.

use rusqlite::Connection;

use super::super::read_fail::read_fail;
use super::super::ReadResult;
use super::{min_closedate, scope_decision_on, scoped, unified_from, Query, ScopeDecision};
use crate::db::{ProfitScope, ProfitUnit};

/// The per-cell aggregate shared by every calendar view: profit sum, trade count, and win
/// count (`pnl > 0`). ONE definition so the daily grid, the hour grid, and the hour-of-day
/// profile can never disagree about what counts as a winning trade.
const CELL_AGG: &str = "COALESCE(SUM(o.pnl), 0), COUNT(*), COALESCE(SUM(o.pnl > 0), 0)";

/// A calendar heatmap cell containing the aggregate for one UTC day or hour.
#[derive(Clone, Debug, Default)]
pub struct DayCell {
    /// Cell start in UTC Unix seconds.
    pub start: i64,
    pub profit: f64,
    pub trades: i64,
    /// Winning trades in the cell; losses equal `trades - wins`.
    pub wins: i64,
}

/// Current Calendar cells and the optional previous-period aggregate from one SQLite snapshot.
#[derive(Clone, Debug, Default)]
pub struct CalendarPeriod {
    /// Daily or hourly cells for the visible Calendar scope.
    pub current: Vec<DayCell>,
    /// Previous-period `(profit, trades, wins)` used by the Month KPI.
    pub previous: Option<(f64, i64, i64)>,
}

/// Read a Calendar period over an existing connection or pinned compound snapshot.
///
/// Args:
///     conn: Existing SQLite connection whose snapshot should serve both periods.
///     q: Visible Calendar scope and time range.
///     previous: Optional comparison scope.
///     hourly: Whether the visible result uses hour buckets.
///
/// Returns:
///     Comparable or empty Calendar data, split-only totals for unsafe raw money, or one
///     classified failure for the pair.
pub(super) fn calendar_period_from(
    conn: &Connection,
    q: &Query,
    previous: Option<&Query>,
    hourly: bool,
) -> ReadResult<ProfitScope<CalendarPeriod>> {
    let mut current_query = q.clone();
    if current_query.from < 0 {
        current_query.from = min_closedate(conn)?;
    }
    let decision = match scope_decision_on(conn, &current_query)? {
        ScopeDecision::Split(totals) => return Ok(ProfitScope::Split(totals)),
        decision => decision,
    };
    let current = if hourly {
        calendar_hours_from(conn, &current_query)?
    } else {
        calendar_cells_from(conn, &current_query)?
    };
    let previous = match previous {
        Some(query) if comparison_is_compatible(conn, &decision, query)? => Some(
            calendar_cells_from(conn, query)?
                .iter()
                .fold((0.0f64, 0i64, 0i64), |total, day| {
                    (
                        total.0 + day.profit,
                        total.1 + day.trades,
                        total.2 + day.wins,
                    )
                }),
        ),
        Some(_) | None => None,
    };
    Ok(scoped(decision, CalendarPeriod { current, previous }))
}

/// Decide whether a Calendar comparison period shares the current profit unit.
///
/// Args:
///     conn: Pinned report snapshot.
///     current: Current-period comparability decision.
///     previous: Previous-period query.
///
/// Returns:
///     `true` for percent mode or the same exact known quote currency, or a classified read
///     failure when the previous-period quote cannot be verified.
fn comparison_is_compatible(
    conn: &Connection,
    current: &ScopeDecision,
    previous: &Query,
) -> ReadResult<bool> {
    Ok(match current {
        ScopeDecision::Comparable(ProfitUnit::Percent) => true,
        ScopeDecision::Comparable(ProfitUnit::Quote(current_quote)) => matches!(
            scope_decision_on(conn, previous)?,
            ScopeDecision::Comparable(ProfitUnit::Quote(previous_quote))
                if *current_quote == previous_quote
        ),
        ScopeDecision::Empty | ScopeDecision::Split(_) => false,
    })
}

/// Dense daily cells for calendar heatmaps (GitHub-style Year or the large Month view).
/// One bucket is one UTC day; `GROUP BY closedate/86400` yields its PnL, trade count, and
/// wins. The range is filled COMPLETELY with empty cells for days without trades so the
/// calendar grid remains regular; unlike [`summary`], it NEVER widens buckets to a week.
/// An empty result means the period has no closed trades or the source schema has not arrived yet;
/// database and query failures remain classified so UI callers can retry transient contention.
///
/// Args:
///     q: Report scope and time range to aggregate.
///
/// Returns:
///     Comparable or empty dense cells, split-only totals, or a classified database read failure.
pub fn calendar_cells(q: &Query) -> ReadResult<ProfitScope<Vec<DayCell>>> {
    let conn = super::super::open_reader()?;
    super::super::with_read_snapshot(&conn, |snapshot| {
        let mut q = q.clone();
        if q.from < 0 {
            q.from = min_closedate(snapshot)?;
        }
        let decision = match scope_decision_on(snapshot, &q)? {
            ScopeDecision::Split(totals) => return Ok(ProfitScope::Split(totals)),
            decision => decision,
        };
        Ok(scoped(decision, calendar_cells_from(snapshot, &q)?))
    })
}

/// Core of [`calendar_cells`] over an existing connection and the entry point for unit tests,
/// which seed an in-memory `orders_rep` and verify bucketing, gaps, and win counts.
///
/// Args:
///     conn: Existing SQLite connection whose snapshot should be queried.
///     q: Report scope and time range to aggregate.
///
/// Returns:
///     Dense daily cells or a classified database read failure.
fn calendar_cells_from(conn: &Connection, q: &Query) -> ReadResult<Vec<DayCell>> {
    const CTX: &str = "analytics: calendar_cells";
    let mut q = q.clone();
    let all_history = q.from < 0;
    if all_history {
        q.from = min_closedate(conn)?;
    }
    let Some(src) = unified_from(conn, &q)? else {
        // Source schemas have not arrived yet, so return an empty calendar (as `summary`
        // returns its default), not an error; otherwise the tab would remain on Loading.
        return Ok(Vec::new());
    };
    // Daily bucket: PnL, trade count, and wins for W/L and win rate.
    let sql = format!(
        "SELECT (o.closedate / 86400) * 86400 AS d,
                {CELL_AGG}
         FROM {src} GROUP BY d ORDER BY d"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .map_err(|e| read_fail(CTX, e))?;
    let mut map: std::collections::HashMap<i64, DayCell> = std::collections::HashMap::new();
    let (mut first, mut last) = (i64::MAX, i64::MIN);
    for row in rows {
        let (d, profit, n, wins) = row.map_err(|e| read_fail(CTX, e))?;
        first = first.min(d);
        last = last.max(d);
        map.insert(
            d,
            DayCell {
                start: d,
                profit,
                trades: n,
                wins,
            },
        );
    }
    if map.is_empty() {
        return Ok(Vec::new()); // A period without trades has an empty calendar.
    }
    // Dense-grid bounds start at the first day with data for All (avoiding years of empty
    // cells since the epoch), or at the requested period's start otherwise. Because `to` is
    // exclusive, the last day is day(to - 1); do not extend into the future.
    let now = crate::util::now_unix_ms_i64() / 1000;
    let today0 = now.div_euclid(86_400) * 86_400;
    let day0 = if all_history {
        first
    } else {
        q.from.div_euclid(86_400) * 86_400
    };
    let last_grid = ((q.to - 1).div_euclid(86_400) * 86_400)
        .min(today0)
        .max(last);
    let day0 = day0.min(last_grid);
    let mut out = Vec::with_capacity((((last_grid - day0) / 86_400) + 1).max(1) as usize);
    let mut t = day0;
    while t <= last_grid {
        out.push(map.remove(&t).unwrap_or(DayCell {
            start: t,
            ..Default::default()
        }));
        t += 86_400;
    }
    Ok(out)
}

/// Hourly cells for the calendar's Day mode: `start` is the UTC hour start, with PnL,
/// trades, and wins. The result is sparse (only hours with trades); the UI builds the 24xN
/// grid. An empty result means no trades or source schema; failures remain classified.
///
/// Args:
///     q: Report scope and one-day time range to aggregate.
///
/// Returns:
///     Comparable or empty sparse cells, split-only totals, or a classified database read failure.
pub fn calendar_hours(q: &Query) -> ReadResult<ProfitScope<Vec<DayCell>>> {
    let conn = super::super::open_reader()?;
    super::super::with_read_snapshot(&conn, |snapshot| {
        let decision = match scope_decision_on(snapshot, q)? {
            ScopeDecision::Split(totals) => return Ok(ProfitScope::Split(totals)),
            decision => decision,
        };
        Ok(scoped(decision, calendar_hours_from(snapshot, q)?))
    })
}

/// Core of [`calendar_hours`] over an existing connection or pinned read snapshot.
///
/// Args:
///     conn: Existing SQLite connection whose snapshot should be queried.
///     q: Report scope and one-day time range to aggregate.
///
/// Returns:
///     Sparse hourly cells or a classified database read failure.
fn calendar_hours_from(conn: &Connection, q: &Query) -> ReadResult<Vec<DayCell>> {
    const CTX: &str = "analytics: calendar_hours";
    let src = match unified_from(conn, q)? {
        Some(s) => s,
        None => return Ok(Vec::new()), // The schema has not arrived yet.
    };
    let sql = format!(
        "SELECT (o.closedate / 3600) * 3600 AS h,
                {CELL_AGG}
         FROM {src} GROUP BY h ORDER BY h"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok(DayCell {
                start: r.get(0)?,
                profit: r.get(1)?,
                trades: r.get(2)?,
                wins: r.get(3)?,
            })
        })
        .map_err(|e| read_fail(CTX, e))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| read_fail(CTX, e))?);
    }
    Ok(out)
}

/// Hour-of-day profile (0..23 UTC): PnL, trades, and wins aggregated across all days in
/// the period. This supplies a cell in the Tuning by-time lower heatmap.
#[derive(Clone, Copy, Debug, Default)]
pub struct HourStat {
    pub profit: f64,
    pub trades: i64,
    pub wins: i64,
}

/// Build hour-of-day profiles for several periods at once (current/week/month/90 days) for
/// the Tuning by-time lower heatmap. One reader and one snapshot serve ALL ranges so columns
/// in the same map see the same data. Core, side, emulator, and strategy filters come from
/// `base`; each range's from/to override the period (`from < 0` means all history). The
/// returned profiles align with `ranges`.
pub fn hourly_profiles(base: &Query, ranges: &[(i64, i64)]) -> ReadResult<Vec<[HourStat; 24]>> {
    let conn = super::super::open_reader()?;
    super::super::with_read_snapshot(&conn, |snapshot| hourly_profiles_on(snapshot, base, ranges))
}

/// Build hour-of-day profiles on an existing connection or shared compound-read snapshot.
///
/// Args:
///     conn: Existing SQLite connection whose snapshot should be queried.
///     base: Shared report scope.
///     ranges: Ordered time ranges to aggregate.
///
/// Returns:
///     Profiles aligned with `ranges`, or a classified read failure.
fn hourly_profiles_on(
    conn: &Connection,
    base: &Query,
    ranges: &[(i64, i64)],
) -> ReadResult<Vec<[HourStat; 24]>> {
    let mut out = Vec::with_capacity(ranges.len());
    for &(from, to) in ranges {
        out.push(hour_profile_one(conn, base, from, to)?);
    }
    Ok(out)
}

/// One hour-of-day profile column for `[from, to)` on an existing snapshot.
///
/// Args:
///     conn: Pinned report snapshot.
///     base: Shared report filters and profit metric.
///     from: Inclusive period start, or a negative all-history sentinel.
///     to: Exclusive period end.
///
/// Returns:
///     Twenty-four comparable hourly aggregates or a classified read failure.
fn hour_profile_one(
    conn: &Connection,
    base: &Query,
    from: i64,
    to: i64,
) -> ReadResult<[HourStat; 24]> {
    const CTX: &str = "analytics: hour_profile";
    let mut q = base.clone();
    q.from = if from < 0 { min_closedate(conn)? } else { from };
    q.to = to;
    if matches!(scope_decision_on(conn, &q)?, ScopeDecision::Split(_)) {
        return Err(super::super::ReadFail::IncomparableQuote);
    }
    let mut prof = [HourStat::default(); 24];
    // Source schemas have not arrived yet, so return an empty profile like summary/calendar.
    let Some(src) = unified_from(conn, &q)? else {
        return Ok(prof);
    };
    // Hour of day comes from the trade OPEN time (`buydate`), matching the schedule and tuner
    // sliders that gate ENTRY. Fall back to `closedate` when the open time is absent (0/NULL).
    // The period itself still uses `closedate`, which defines the analysis window.
    let sql = format!(
        "SELECT ((COALESCE(NULLIF(o.buydate, 0), o.closedate) % 86400) / 3600) AS h,
                {CELL_AGG}
         FROM {src} GROUP BY h ORDER BY h"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .map_err(|e| read_fail(CTX, e))?;
    for row in rows {
        let (h, profit, trades, wins) = row.map_err(|e| read_fail(CTX, e))?;
        if (0..24).contains(&h) {
            prof[h as usize] = HourStat {
                profit,
                trades,
                wins,
            };
        }
    }
    Ok(prof)
}

#[cfg(test)]
mod tests;
