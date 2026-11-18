//! Manual and daily snapshots of the order-trace archive (`order_traces.sqlite`).
//!
//! Modelled on `strat_db::backup`: the same 12:00 UTC due slot, the same seven-period retention,
//! the same staging-then-rename publication through `backup_store`. Unlike the strategy domain,
//! there is no readiness gate — a trace archive is complete row by row, so whatever the writer has
//! committed is a valid recovery point.
//!
//! The copy is never a raw file copy: a dedicated read-only connection runs `VACUUM INTO`, which
//! reads one consistent snapshot through the WAL while the writer thread keeps committing.
//!
//! No restore path exists: the strategy domain exposes none either, so a snapshot is recovered by
//! hand (copy its `order_traces.sqlite` over the live file while the terminal is closed).

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use crate::backup_store::sqlite::{self, snapshot_stamp};
use crate::backup_store::{ExactPublication, SnapshotStore};
use crate::backups::{DAY_MS, DueOutcome, RETAIN_PERIODS, due_slot_ms};
use crate::config::paths;
use crate::util::utc_stamp_compact;

/// Name of the SQLite file inside every completed snapshot directory.
const DATABASE_NAME: &str = paths::ORDER_TRACES_DB_FILE;
/// Marker written only after SQLite completes a valid staged copy.
const COMPLETION_NAME: &str = ".complete";
/// Versioned marker contents distinguish application snapshots from lookalike user directories.
const COMPLETION_CONTENT: &[u8] = b"moonterminal-order-traces-backup-v1\n";
/// Complete file set required in every removable order-trace snapshot.
const SNAPSHOT_FILES: [&str; 2] = [DATABASE_NAME, COMPLETION_NAME];
/// How long the snapshot read waits on a writer holding an exclusive lock (a checkpoint).
const BUSY_TIMEOUT: Duration = Duration::from_secs(30);

/// Return whether `dir` is a completed snapshot with a nonempty regular database file.
fn is_completed_snapshot(dir: &Path) -> bool {
    sqlite::is_completed_snapshot(dir, DATABASE_NAME, COMPLETION_NAME, COMPLETION_CONTENT)
}

/// Copy the live archive into `dst` through a read-only connection.
///
/// `VACUUM INTO` runs inside one read transaction, so it sees every frame the writer committed
/// to the WAL up to that point and none after; the source file is never copied byte for byte.
///
/// Args:
///     src: Live `order_traces.sqlite`.
///     dst: Not-yet-existing destination file inside a staging directory.
///
/// Raises:
///     An error when the source cannot be opened read-only or the copy fails.
fn copy_consistent(src: &Path, dst: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        !dst.exists(),
        "order-trace backup file already exists: {}",
        dst.display()
    );
    let conn = Connection::open_with_flags(
        src,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let dst_sql = dst.to_string_lossy().replace('\'', "''");
    conn.execute(&format!("VACUUM INTO '{dst_sql}'"), [])?;
    Ok(())
}

/// Assemble one SQLite snapshot in a unique staging directory.
fn assemble_snapshot<'a>(
    src: &Path,
    backups: &'a Path,
) -> anyhow::Result<(SnapshotStore<'a>, PathBuf)> {
    let store = SnapshotStore::new(backups, &SNAPSHOT_FILES, true);
    let staging = store.create_staging()?;
    if let Err(error) = copy_consistent(src, &staging.join(DATABASE_NAME)) {
        store.discard_staging(&staging);
        return Err(error);
    }
    if let Err(error) = std::fs::write(staging.join(COMPLETION_NAME), COMPLETION_CONTENT) {
        store.discard_staging(&staging);
        return Err(error.into());
    }
    if !is_completed_snapshot(&staging) {
        store.discard_staging(&staging);
        anyhow::bail!("staged order-trace backup failed completion validation")
    }
    Ok((store, staging))
}

/// Remove completed order-trace snapshots older than the latest seven UTC-noon periods.
fn prune(backups: &Path, current_slot_ms: i64) -> usize {
    let cutoff = utc_stamp_compact(current_slot_ms - (RETAIN_PERIODS - 1) * DAY_MS);
    let removed = SnapshotStore::new(backups, &SNAPSHOT_FILES, true).prune_where(
        |name| snapshot_stamp(name).is_some(),
        |name| snapshot_stamp(name).is_some_and(|stamp| stamp < cutoff.as_str()),
        is_completed_snapshot,
    );
    if removed > 0 {
        log::info!("order-trace backups: pruned {removed} snapshots older than seven UTC periods");
    }
    removed
}

/// Return whether `src` is present as a regular file; `Ok(false)` when it does not exist yet.
///
/// Raises:
///     An error when the path exists but is not a regular file or cannot be inspected.
fn source_present(src: &Path) -> anyhow::Result<bool> {
    match std::fs::symlink_metadata(src) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => anyhow::bail!(
            "order-trace database source is not a file: {}",
            src.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(anyhow::Error::from(error).context(format!(
            "order-trace database is not readable: {}",
            src.display()
        ))),
    }
}

/// Back up the latest scheduled slot, or report that the lazy archive is still absent.
fn backup_due_into(src: &Path, backups: &Path, now_ms: i64) -> anyhow::Result<DueOutcome> {
    let slot = due_slot_ms(now_ms);
    let stamp = utc_stamp_compact(slot);
    let destination = backups.join(&stamp);
    if is_completed_snapshot(&destination) {
        prune(backups, slot);
        return Ok(DueOutcome::Current(destination));
    }
    match std::fs::symlink_metadata(&destination) {
        Ok(_) => anyhow::bail!(
            "scheduled order-trace backup path is occupied by an incomplete or foreign entry: {}",
            destination.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if !source_present(src)? {
        return Ok(DueOutcome::SourceMissing);
    }
    let (store, staging) = assemble_snapshot(src, backups)?;
    let published = store.publish_exact(&staging, &stamp, is_completed_snapshot)?;
    prune(backups, slot);
    Ok(match published {
        ExactPublication::Created(path) => DueOutcome::Created(path),
        ExactPublication::Existing(path) => DueOutcome::Current(path),
    })
}

/// Back up the latest due order-trace slot at the canonical paths.
pub(crate) fn backup_due_at(now_ms: i64) -> anyhow::Result<DueOutcome> {
    backup_due_into(
        &paths::order_traces_db_path(),
        &paths::order_traces_backups_dir(),
        now_ms,
    )
}

#[cfg(test)]
mod tests;
