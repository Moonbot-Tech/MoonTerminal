//! Calendar heatmap cells and hour-of-day profiles.

use rusqlite::Connection;

use super::super::read_fail::read_fail;
use super::super::ReadResult;
use super::{min_closedate, unified_from, Query};

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

/// Dense daily cells for calendar heatmaps (GitHub-style Year or the large Month view).
/// One bucket is one UTC day; `GROUP BY closedate/86400` yields its PnL, trade count, and
/// wins. The range is filled COMPLETELY with empty cells for days without trades so the
/// calendar grid remains regular; unlike [`summary`], it NEVER widens buckets to a week.
/// `None` means a database, schema, or query read failed; `Some(empty)` means the period has
/// no closed trades or the source schema has not arrived yet.
///
/// NOTE: this surface still collapses a read failure into `None`, the pattern
/// the rest of this module moved away from. Converting it is left to the owners
/// of the calendar feature rather than rewritten here; the `.ok()?` calls below
/// only adapt it to the now-fallible helpers.
pub fn calendar_cells(q: &Query) -> Option<Vec<DayCell>> {
    calendar_cells_from(&super::super::open_reader().ok()?, q)
}

/// Core of [`calendar_cells`] over an existing connection and the entry point for unit tests,
/// which seed an in-memory `orders_rep` and verify bucketing, gaps, and win counts.
fn calendar_cells_from(conn: &Connection, q: &Query) -> Option<Vec<DayCell>> {
    let mut q = q.clone();
    let all_history = q.from < 0;
    if all_history {
        q.from = min_closedate(conn).ok()?;
    }
    let Some(src) = unified_from(conn, &q).ok()? else {
        // Source schemas have not arrived yet, so return an empty calendar (as `summary`
        // returns its default), NOT None; otherwise the tab would remain on Loading.
        return Some(Vec::new());
    };
    // Daily bucket: PnL, trade count, and wins for W/L and win rate.
    let sql = format!(
        "SELECT (o.closedate / 86400) * 86400 AS d,
                COALESCE(SUM(o.pnl), 0), COUNT(*), COALESCE(SUM(o.pnl > 0), 0)
         FROM {src} GROUP BY d ORDER BY d"
    );
    let mut stmt = conn.prepare(&sql).ok()?;
    let rows = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .ok()?;
    let mut map: std::collections::HashMap<i64, DayCell> = std::collections::HashMap::new();
    let (mut first, mut last) = (i64::MAX, i64::MIN);
    for (d, profit, n, wins) in rows.flatten() {
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
        return Some(Vec::new()); // A period without trades has an empty calendar.
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
    Some(out)
}

/// Hourly cells for the calendar's Day mode: `start` is the UTC hour start, with PnL,
/// trades, and wins. The result is sparse (only hours with trades); the UI builds the 24xN
/// grid. `None` means the DB or query is unavailable; `Some(empty)` means no trades or schema.
pub fn calendar_hours(q: &Query) -> Option<Vec<DayCell>> {
    let conn = super::super::open_reader().ok()?;
    let src = match unified_from(&conn, q) {
        Ok(Some(s)) => s,
        Ok(None) => return Some(Vec::new()), // The schema has not arrived yet.
        Err(_) => return None,
    };
    let sql = format!(
        "SELECT (o.closedate / 3600) * 3600 AS h,
                COALESCE(SUM(o.pnl), 0), COUNT(*), COALESCE(SUM(o.pnl > 0), 0)
         FROM {src} GROUP BY h ORDER BY h"
    );
    let mut stmt = conn.prepare(&sql).ok()?;
    let out = stmt
        .query_map(rusqlite::params![q.from, q.to], |r| {
            Ok(DayCell {
                start: r.get(0)?,
                profit: r.get(1)?,
                trades: r.get(2)?,
                wins: r.get(3)?,
            })
        })
        .ok()?
        .flatten()
        .collect();
    Some(out)
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
    let snap = super::super::read_snapshot(&conn)?;
    let conn = &*snap;
    let mut out = Vec::with_capacity(ranges.len());
    for &(from, to) in ranges {
        out.push(hour_profile_one(conn, base, from, to)?);
    }
    Ok(out)
}

/// One hour-of-day profile column for `[from, to)` on an existing snapshot.
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
                COALESCE(SUM(o.pnl), 0), COUNT(*), COALESCE(SUM(o.pnl > 0), 0)
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
