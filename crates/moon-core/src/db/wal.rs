//! The WAL settings every SQLite store in this crate opens with, and the one compaction they share.
//!
//! # Why a size limit
//!
//! SQLite never shrinks a `-wal` file on its own: a checkpoint copies the frames back into the
//! database and the next writer starts over at the head of the file, but the file keeps the
//! length of the largest transaction it ever held. One `VACUUM` rewrites the whole database
//! through the WAL, so after it the sidecar is as large as the database and stays that way until
//! the last connection closes — measured 2026-09-25 on a running terminal: `trades.sqlite` 220 MB
//! beside a 225 MB `-wal`, `valuation.sqlite` 104 MB beside 133 MB. `journal_size_limit` makes
//! SQLite truncate the file back to the limit each time a writer restarts it.
//!
//! # Why the checkpoint comes after the `VACUUM`
//!
//! The frames a `VACUUM` writes are the ones to move and cut; a `TRUNCATE` checkpoint run before
//! it empties a WAL the `VACUUM` then fills again.

use rusqlite::Connection;

/// Bytes a `-wal` file is cut back to once a checkpoint lets a writer restart it. Above every
/// store's own checkpoint cadence — `valuation` checkpoints every 8 192 pages, 32 MB — so an
/// ordinary write cycle never truncates and regrows the file; only a transaction larger than
/// that leaves a sidecar to cut.
pub(crate) const WAL_SIZE_LIMIT_BYTES: i64 = 64 * 1024 * 1024;

/// Put a freshly opened connection in WAL mode with the size limit.
///
/// Args:
///     conn: The store's writer connection, before any statement of its own.
///
/// Errors:
///     Any SQLite failure setting either pragma.
pub(crate) fn enable(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "journal_size_limit", WAL_SIZE_LIMIT_BYTES)?;
    Ok(())
}

/// Compact the database and cut its `-wal` file back to nothing.
///
/// The checkpoint's own result is not an error: a reader still holding a snapshot keeps the
/// frames it reads, the checkpoint reports busy, and the size limit cuts the file at the next
/// restart instead.
///
/// Args:
///     conn: A connection outside any transaction.
///
/// Errors:
///     The `VACUUM`'s failure; the database is then as it was.
pub(crate) fn vacuum(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("VACUUM;")?;
    let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    Ok(())
}

#[cfg(test)]
mod tests;
