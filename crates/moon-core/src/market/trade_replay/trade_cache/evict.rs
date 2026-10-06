//! Eviction for free disk space: the station's tape file gives room back to the server
//! (`docs-internal/STATION.md` §1 п. 7, §4.9 п. 4).
//!
//! A row deleted from SQLite leaves a free page inside the file, not free space on the disk: the
//! file keeps its length, and a disk short of space stays short however much is deleted. So a file
//! that evicts for the disk is kept with `auto_vacuum = INCREMENTAL`, and every eviction hands its
//! freed pages back to the filesystem (`incremental_vacuum`, then a checkpoint that cuts the file
//! in WAL mode) — the next look at the disk sees the room it made. A file that is not in that mode
//! (its one `VACUUM` at open failed — it needs about the file's size free) evicts nothing: rows
//! deleted there would give the disk nothing back and only lose the tape. The switch is not tried
//! again here: a `VACUUM` on a disk already short is the last thing it needs; the next start tries.

use rusqlite::Connection;

/// `PRAGMA auto_vacuum` of a file that gives pages back on request.
const INCREMENTAL: i64 = 2;

/// Make `conn`'s file able to give pages back: `auto_vacuum = INCREMENTAL`, and a `VACUUM` once
/// for a file made without it — SQLite takes the mode only when it rebuilds the file.
///
/// Errors:
///     Any SQLite failure; the file then evicts nothing until a later start switches it.
pub(super) fn make_shrinkable(conn: &Connection) -> rusqlite::Result<()> {
    if auto_vacuum(conn)? != INCREMENTAL {
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
        crate::db::wal::vacuum(conn)?;
    }
    Ok(())
}

fn auto_vacuum(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA auto_vacuum", [], |r| r.get(0))
}

/// What one eviction did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Evicted {
    /// Rows gone and their pages handed back; `shrunk` is whether the checkpoint that cuts the
    /// file ran (a reader kept it busy otherwise — the next one cuts it).
    Freed { held: i64, shrunk: bool },
    /// The file cannot give pages back, so nothing was deleted.
    NotShrinkable,
}

/// Drop the spans written longest ago until `bytes` packed bytes are gone — or nothing is left —
/// and give the freed pages back to the filesystem. The deletes and the page hand-back are one
/// transaction: rows never go without their pages.
///
/// Args:
///     conn: The open connection, outside any transaction.
///     held: Packed bytes the file holds now.
///     bytes: Packed bytes to free.
pub(super) fn evict_oldest(conn: &Connection, held: i64, bytes: i64) -> rusqlite::Result<Evicted> {
    if auto_vacuum(conn)? != INCREMENTAL {
        return Ok(Evicted::NotShrinkable);
    }
    let tx = conn.unchecked_transaction()?;
    let after = super::trim_to_ceiling(&tx, held, Some(held.saturating_sub(bytes).max(0)), None)?;
    {
        let mut stmt = tx.prepare("PRAGMA incremental_vacuum")?;
        let mut rows = stmt.query([])?;
        while rows.next()?.is_some() {}
    }
    tx.commit()?;
    // `(busy, log frames, checkpointed frames)`: busy is a row, not an error. The connection's
    // busy timeout already waits for a reader (the station's pull of the tape); one still reading
    // after it leaves the cut to the next checkpoint.
    let shrunk = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
            r.get::<_, i64>(0)
        })
        .is_ok_and(|busy| busy == 0);
    Ok(Evicted::Freed {
        held: after,
        shrunk,
    })
}

#[cfg(test)]
mod tests;
