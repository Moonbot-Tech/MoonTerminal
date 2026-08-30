//! Durable store for measured per-core clock offsets, backing [`crate::db::report_axis`].
//!
//! One append-only segment per adopted offset change: `report_axis`'s own module doc forbids
//! retroactively correcting a stored value, so a new measurement opens a new row rather than
//! rewriting an earlier one. [`load_all`] is the money-critical read: a skewed core's rows are
//! wrong money if the offset silently collapses to empty on a read failure, so absence and
//! failure are distinguished honestly rather than folded into the same empty result.

use std::collections::HashMap;
use std::sync::Arc;

use rusqlite::Connection;

use crate::db::read_fail::read_fail;
use crate::db::report_axis::{MAX_OFFSET_SECS, MIN_OFFSET_SECS, OffsetSegment};
use crate::db::{FailKind, ReadFail, ReadResult};

const TABLE: &str = "core_time_offset";

/// Create the offset-segment table if it does not already exist.
///
/// Args:
///     conn: Open writer connection that owns the replica schema.
///
/// Returns:
///     Nothing, or the SQLite failure from creating the table.
pub fn ensure_table(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS core_time_offset (
            core_uid    INTEGER NOT NULL,
            from_utc    INTEGER NOT NULL,
            offset_secs INTEGER NOT NULL,
            observed_at INTEGER NOT NULL,
            source      TEXT    NOT NULL,
            PRIMARY KEY (core_uid, from_utc)
        )",
        [],
    )?;
    Ok(())
}

/// Idempotently store one adopted offset segment.
///
/// Args:
///     core_uid: Stable uid of the core the segment belongs to.
///     from_utc: True-UTC instant, in seconds, from which `offset_secs` applies.
///     offset_secs: Seconds east of UTC on the core's clock.
///     observed_at: True-UTC instant, in seconds, the adoption itself happened.
///     source: Label of the estimator that adopted this segment.
///
/// Returns:
///     Nothing, or the SQLite failure from upserting the segment.
pub fn store_segment(
    conn: &Connection,
    core_uid: u64,
    from_utc: i64,
    offset_secs: i32,
    observed_at: i64,
    source: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO {TABLE} (core_uid, from_utc, offset_secs, observed_at, source) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(core_uid, from_utc) DO UPDATE SET \
             offset_secs=excluded.offset_secs, observed_at=excluded.observed_at, \
             source=excluded.source"
        ),
        rusqlite::params![core_uid as i64, from_utc, offset_secs, observed_at, source],
    )?;
    Ok(())
}

/// Read the offset currently IN FORCE for one core: the newest segment's value.
///
/// The durable answer to "has anything actually changed", and the only one that survives a
/// restart. [`crate::session::core_time_offset::OffsetEstimator`] dedupes re-confirmations of the
/// same value, but it holds that memory on the FEED CONNECTION: a restart or a hard reconnect
/// gives every core a fresh estimator that re-adopts its unchanged offset from scratch. Since an
/// adoption invalidates caches and DELETES that core's whole `trade_values` partition, the
/// writer has to be able to tell a re-confirmation from a change without trusting the sender.
///
/// Newest by `from_utc`, matching [`crate::db::report_axis::ReportAxis`]'s own segment selection,
/// which scans the sorted list and takes the last segment starting at or before the value.
///
/// Deliberately FAIL-OPEN, unlike [`load_all`], and the signature is what enforces it: this
/// returns a bare `Option` rather than a `Result` so no caller can ever propagate a read failure
/// out of it. The two directions are not symmetric. `load_all` decides what the axis IS, where an
/// empty answer is the wrong-money answer on a skewed core. This decides only whether to SKIP
/// redundant work, and it is read inside the SOLE report writer's own transaction — so a
/// propagated error would fail the whole owned batch and put that writer into its retry-and-stop
/// path, taking every unrelated replica upsert in the batch with it. Answering `None` instead
/// costs one redundant rescan and leaves the adoption behaving exactly as it did before this
/// guard existed.
///
/// Args:
///     conn: Open writer connection or report reader.
///     core_uid: Stable uid of the core to read.
///
/// Returns:
///     The newest segment's offset in seconds, or `None` when the table is absent, holds no
///     segment for this core, holds an `offset_secs` value that does not fit `i32`, or cannot be
///     read at all.
pub fn latest_offset(conn: &Connection, core_uid: u64) -> Option<i32> {
    // No `table_exists` probe, unlike [`load_all`]. That read has to distinguish an ABSENT table
    // from an unreadable one; this one folds both into `None` anyway, so the probe would only add
    // a second `sqlite_master` query per message on the path this guard exists to make cheap.
    //
    // `.ok()` and not `.optional()`: rusqlite's `optional()` folds ONLY `QueryReturnedNoRows` into
    // `None` and propagates every other query or row-decoding error — a missing table among them —
    // which is exactly the propagation this function must not have.
    conn.query_row(
        &format!(
            "SELECT offset_secs FROM {TABLE} WHERE core_uid=?1 \
             ORDER BY from_utc DESC LIMIT 1"
        ),
        rusqlite::params![core_uid as i64],
        |row| row.get::<_, i64>(0),
    )
    .ok()
    .and_then(|value| i32::try_from(value).ok())
}

/// Load every core's offset segments, sorted ascending by `from_utc` as
/// [`crate::db::report_axis::ReportAxis::from_measured`] expects.
///
/// FAILS CLOSED: an absent table is a fresh install and returns empty, but an unreadable or
/// self-inconsistent table returns [`ReadFail::Failed`] rather than silently collapsing to the
/// same empty result -- on a skewed core the empty (identity) axis is the wrong-money axis, and
/// this is the one seam that must never confuse "never measured" with "measurement unreadable".
///
/// Args:
///     conn: Open report reader or pinned snapshot.
///
/// Returns:
///     Every core's ordered segments, an empty map for an absent table, or a classified read
///     failure.
pub fn load_all(conn: &Connection) -> ReadResult<HashMap<u64, Vec<OffsetSegment>>> {
    const CTX: &str = "core_time_offset: load_all";

    if !super::table_exists(conn, TABLE) {
        return Ok(HashMap::new());
    }

    let mut stmt = conn
        .prepare(&format!(
            "SELECT core_uid, from_utc, offset_secs, \
             typeof(core_uid), typeof(from_utc), typeof(offset_secs), \
             typeof(observed_at), typeof(source) \
             FROM {TABLE} ORDER BY core_uid, from_utc"
        ))
        .map_err(|e| read_fail(CTX, e))?;
    let mut rows = stmt.query([]).map_err(|e| read_fail(CTX, e))?;

    let mut out: HashMap<u64, Vec<OffsetSegment>> = HashMap::new();
    let mut last_key: Option<(i64, i64)> = None;
    loop {
        let row = match rows.next() {
            Ok(Some(r)) => r,
            Ok(None) => break,
            Err(e) => return Err(read_fail(CTX, e)),
        };
        let core_uid: i64 = row.get(0).map_err(|e| read_fail(CTX, e))?;
        let from_utc: i64 = row.get(1).map_err(|e| read_fail(CTX, e))?;
        let offset_secs: i64 = row.get(2).map_err(|e| read_fail(CTX, e))?;
        let types: [String; 5] = [
            row.get(3).map_err(|e| read_fail(CTX, e))?,
            row.get(4).map_err(|e| read_fail(CTX, e))?,
            row.get(5).map_err(|e| read_fail(CTX, e))?,
            row.get(6).map_err(|e| read_fail(CTX, e))?,
            row.get(7).map_err(|e| read_fail(CTX, e))?,
        ];
        const EXPECTED: [&str; 5] = ["integer", "integer", "integer", "integer", "text"];
        if types.iter().zip(EXPECTED).any(|(t, want)| t != want) {
            return Err(inconsistent(
                "column typeof does not match the declared schema",
            ));
        }

        let Ok(offset_secs) = i32::try_from(offset_secs) else {
            return Err(inconsistent("offset_secs does not fit the declared range"));
        };
        if !(MIN_OFFSET_SECS..=MAX_OFFSET_SECS).contains(&offset_secs) {
            return Err(inconsistent("offset_secs outside the plausible zone band"));
        }
        // ORDER BY already sorts ascending; a same-core row that does not strictly advance
        // `from_utc` is either a duplicate key or an out-of-order scan, neither possible from a
        // healthy PRIMARY KEY (core_uid, from_utc) and therefore a corruption signal.
        if let Some((last_core, last_from_utc)) = last_key {
            if last_core == core_uid && last_from_utc >= from_utc {
                return Err(inconsistent(
                    "duplicate or non-ascending from_utc for one core",
                ));
            }
        }
        last_key = Some((core_uid, from_utc));

        out.entry(core_uid as u64).or_default().push(OffsetSegment {
            from_utc,
            offset_secs,
        });
    }
    Ok(out)
}

/// Build the self-inconsistency verdict [`load_all`] fails closed with, logging the detail once.
///
/// Args:
///     detail: The violated table invariant.
///
/// Returns:
///     A corruption-classified read failure carrying the invariant detail.
fn inconsistent(detail: &str) -> ReadFail {
    log::warn!("отчёты(core_time_offset): реплика самопротиворечива ({detail})");
    ReadFail::Failed {
        kind: FailKind::Corrupt,
        msg: Arc::from(format!(
            "core_time_offset replica is self-inconsistent: {detail}"
        )),
    }
}

#[cfg(test)]
mod tests;
