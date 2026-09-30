//! Which closed trades a terminal asks its station's traces for (STATION.md §4.4, §4.9): the
//! station, connected all the time, archived traces at every close — a terminal that was off, or a
//! core whose archive no longer holds the trade, lacks them.

use std::collections::BTreeMap;

use rusqlite::{Connection, params};

use super::{BACKFILL_DEPTH, CLOCK_SLACK, replica_columns};
use crate::db::read_fail::read_fail;
use crate::db::{ReadFail, ReadResult};

/// Most trades one listing names: a month of a busy fleet, and one station request per
/// [`crate::station_api::MAX_TRACE_UIDS`] of them.
pub const STATION_LIMIT: usize = 4_000;

/// The closed trades of the last [`BACKFILL_DEPTH`] with no trace lines filed here — never asked,
/// or answered empty by the core (the station may have archived what the core no longer holds),
/// newest close first, grouped by core.
///
/// Args:
///     now_ms: Terminal wall clock, Unix ms.
///
/// Returns:
///     `core_uid` → its `ReportUID`s, at most [`STATION_LIMIT`] in all.
pub fn lacking_lines(now_ms: i64) -> ReadResult<BTreeMap<u64, Vec<i64>>> {
    const CTX: &str = "order traces: attach for the station";
    let conn = crate::db::open_reader()?;
    let path = crate::config::paths::order_traces_db_path();
    let attached = path.exists();
    if attached {
        conn.execute(
            "ATTACH DATABASE ?1 AS traces",
            [path.to_string_lossy().as_ref()],
        )
        .map_err(|e| read_fail(CTX, e))?;
    }
    lacking_lines_on(&conn, attached, now_ms)
}

/// [`lacking_lines`] on a report reader, with the store attached as `traces` when `attached`.
pub(super) fn lacking_lines_on(
    conn: &Connection,
    attached: bool,
    now_ms: i64,
) -> ReadResult<BTreeMap<u64, Vec<i64>>> {
    const CTX: &str = "order traces: station listing";
    let cols = replica_columns(conn)?;
    if !["reportuid", "closedate", "core_uid"]
        .iter()
        .all(|c| cols.iter().any(|have| have == c))
    {
        return Err(ReadFail::NotReady);
    }
    let deleted_filter = if cols.iter().any(|c| c == "deleted") {
        "AND COALESCE(r.deleted, 0) = 0 "
    } else {
        ""
    };
    let lines_filter = if attached {
        "AND NOT EXISTS (SELECT 1 FROM traces.trace_answers a \
         WHERE a.core_uid = r.core_uid AND a.report_uid = r.reportuid AND a.line_count > 0) "
    } else {
        ""
    };
    // Raw `closedate` in the core's clock against a UTC cutoff widened by a day, as the backfill
    // listing does.
    let cutoff_secs = (now_ms / 1000)
        .saturating_sub(BACKFILL_DEPTH.as_secs() as i64)
        .saturating_sub(CLOCK_SLACK.as_secs() as i64);
    let sql = format!(
        "SELECT r.core_uid, r.reportuid FROM orders_rep r \
         WHERE r.reportuid IS NOT NULL AND r.reportuid <> 0 \
         AND r.closedate IS NOT NULL AND r.closedate > 0 AND r.closedate >= ?1 \
         {deleted_filter}{lines_filter}\
         ORDER BY r.closedate DESC LIMIT ?2"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map(params![cutoff_secs, STATION_LIMIT as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })
        .map_err(|e| read_fail(CTX, e))?;
    let mut out: BTreeMap<u64, Vec<i64>> = BTreeMap::new();
    for row in rows {
        let (core, uid) = row.map_err(|e| read_fail(CTX, e))?;
        out.entry(core as u64).or_default().push(uid);
    }
    Ok(out)
}
