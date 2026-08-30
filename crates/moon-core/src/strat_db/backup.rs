//! Manual and daily snapshots of the authoritative strategy database.
//!
//! Scheduled snapshots are anchored to 12:00 UTC. An inherited database checks the latest due slot
//! at startup; a fresh database performs the same catch-up immediately after lazy initialization.
//! SQLite writes each consistent copy into a process-owned staging directory; one directory rename
//! then publishes the completed snapshot without exposing a partial database.

use std::path::{Path, PathBuf};
use std::{fs::File, io::Read as _};

use crate::backup_store::{ExactPublication, SnapshotStore, timestamp_prefix};
use crate::backups::{DAY_MS, DueOutcome, RETAIN_PERIODS, due_slot_ms};
use crate::config::paths;
use crate::util::{now_unix_ms_i64, utc_stamp_compact};

/// Name of the SQLite file inside every completed snapshot directory.
const DATABASE_NAME: &str = paths::STRATEGIES_DB_FILE;
/// Marker written only after SQLite completes a valid staged copy.
const COMPLETION_NAME: &str = ".complete";
/// Versioned marker contents distinguish application snapshots from lookalike user directories.
const COMPLETION_CONTENT: &[u8] = b"moonterminal-strategy-backup-v1\n";
/// Complete file set required in every removable strategy snapshot.
const SNAPSHOT_FILES: [&str; 2] = [DATABASE_NAME, COMPLETION_NAME];
/// Return the fixed timestamp portion of an application-owned snapshot directory name.
fn snapshot_stamp(name: &str) -> Option<&str> {
    let stamp = timestamp_prefix(name)?;
    let digits = |s: &str| s.bytes().all(|byte| byte.is_ascii_digit());
    let valid_suffix = match name.len() {
        15 => true,
        22 => name.get(15..) == Some("-manual"),
        25 => name.get(15..23) == Some("-manual-") && name.get(23..).is_some_and(digits),
        _ => false,
    };
    valid_suffix.then_some(stamp)
}

/// Return whether a regular file begins with SQLite's fixed 16-byte format signature.
fn has_sqlite_header(path: &Path) -> bool {
    let mut header = [0u8; 16];
    File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok()
        && &header == b"SQLite format 3\0"
}

/// Return whether `dir` is a completed snapshot with a nonempty regular database file.
fn is_completed_snapshot(dir: &Path) -> bool {
    let Ok(dir_meta) = std::fs::symlink_metadata(dir) else {
        return false;
    };
    if !dir_meta.is_dir() || dir_meta.file_type().is_symlink() {
        return false;
    }
    let database = dir.join(DATABASE_NAME);
    let valid_database = std::fs::symlink_metadata(&database)
        .map(|meta| meta.is_file() && !meta.file_type().is_symlink() && meta.len() >= 16)
        .unwrap_or(false)
        && has_sqlite_header(&database);
    let marker = dir.join(COMPLETION_NAME);
    let valid_marker = std::fs::symlink_metadata(&marker)
        .map(|meta| meta.is_file() && !meta.file_type().is_symlink())
        .unwrap_or(false)
        && std::fs::read(marker)
            .map(|bytes| bytes == COMPLETION_CONTENT)
            .unwrap_or(false);
    valid_database && valid_marker
}

/// Assemble one SQLite snapshot in a unique staging directory.
fn assemble_snapshot<'a>(
    src: &Path,
    backups: &'a Path,
) -> anyhow::Result<(SnapshotStore<'a>, PathBuf)> {
    let store = SnapshotStore::new(backups, &SNAPSHOT_FILES, true);
    let staging = store.create_staging()?;
    let staged_db = staging.join(DATABASE_NAME);
    if let Err(error) = crate::db::maint::backup_db(src, &staged_db) {
        store.discard_staging(&staging);
        return Err(error);
    }
    if let Err(error) = std::fs::write(staging.join(COMPLETION_NAME), COMPLETION_CONTENT) {
        store.discard_staging(&staging);
        return Err(error.into());
    }
    if !is_completed_snapshot(&staging) {
        store.discard_staging(&staging);
        anyhow::bail!("staged strategy backup failed completion validation")
    }
    Ok((store, staging))
}

/// Create and publish a distinct manual snapshot.
fn create_distinct_snapshot(src: &Path, backups: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let (store, staging) = assemble_snapshot(src, backups)?;
    store.publish_distinct(&staging, name)
}

/// Remove completed strategy snapshots older than the latest seven UTC-noon periods.
fn prune(backups: &Path, current_slot_ms: i64) -> usize {
    let cutoff = utc_stamp_compact(current_slot_ms - (RETAIN_PERIODS - 1) * DAY_MS);
    let removed = SnapshotStore::new(backups, &SNAPSHOT_FILES, true).prune_where(
        |name| snapshot_stamp(name).is_some(),
        |name| snapshot_stamp(name).is_some_and(|stamp| stamp < cutoff.as_str()),
        is_completed_snapshot,
    );
    if removed > 0 {
        log::info!("strategy backups: pruned {removed} snapshots older than seven UTC periods");
    }
    removed
}

/// Return the manual snapshot namespace for one UTC instant.
fn manual_snapshot_name(now_ms: i64) -> String {
    format!("{}-manual", utc_stamp_compact(now_ms))
}

/// Create one distinct manual snapshot at an injected time and path.
fn backup_manual_at(src: &Path, backups: &Path, now_ms: i64) -> anyhow::Result<PathBuf> {
    let published = create_distinct_snapshot(src, backups, &manual_snapshot_name(now_ms))?;
    prune(backups, due_slot_ms(now_ms));
    Ok(published)
}

/// Back up the latest scheduled slot, or report that the lazy database is still absent.
fn backup_due_into(
    src: &Path,
    backups: &Path,
    now_ms: i64,
    topology_generation: u64,
    publish_if_current: impl FnOnce(
        u64,
        &SnapshotStore<'_>,
        &Path,
        &str,
    ) -> anyhow::Result<Option<ExactPublication>>,
) -> anyhow::Result<DueOutcome> {
    let slot = due_slot_ms(now_ms);
    let stamp = utc_stamp_compact(slot);
    let destination = backups.join(&stamp);
    if is_completed_snapshot(&destination) {
        prune(backups, slot);
        return Ok(DueOutcome::Current(destination));
    }
    match std::fs::symlink_metadata(&destination) {
        Ok(_) => anyhow::bail!(
            "scheduled strategy backup path is occupied by an incomplete or foreign entry: {}",
            destination.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    match std::fs::symlink_metadata(src) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => anyhow::bail!("strategy database source is not a file: {}", src.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DueOutcome::SourceMissing);
        }
        Err(error) => {
            return Err(anyhow::Error::from(error).context(format!(
                "strategy database is not readable: {}",
                src.display()
            )));
        }
    }
    let (store, staging) = assemble_snapshot(src, backups)?;
    let Some(published) = publish_if_current(topology_generation, &store, &staging, &stamp)? else {
        store.discard_staging(&staging);
        return Ok(DueOutcome::Pending);
    };
    prune(backups, slot);
    Ok(match published {
        ExactPublication::Created(path) => DueOutcome::Created(path),
        ExactPublication::Existing(path) => DueOutcome::Current(path),
    })
}

/// Back up the latest due strategy slot for one ready topology generation.
pub(crate) fn backup_due_at(now_ms: i64, topology_generation: u64) -> anyhow::Result<DueOutcome> {
    backup_due_into(
        &paths::strategies_db_path(),
        &paths::strategies_backups_dir(),
        now_ms,
        topology_generation,
        |generation, store, staging, stamp| {
            crate::backups::with_current_strategy_topology(generation, || {
                store.publish_exact(staging, stamp, is_completed_snapshot)
            })
            .transpose()
        },
    )
}

/// Return whether the existing strategy database contains at least one durable head row.
pub(crate) fn source_has_strategy_rows() -> bool {
    let path = paths::strategies_db_path();
    source_has_strategy_rows_at(&path)
}

/// Probe one database path read-only for at least one durable strategy head row.
fn source_has_strategy_rows_at(path: &Path) -> bool {
    if !std::fs::symlink_metadata(path)
        .map(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return false;
    }
    super::open_ro(path)
        .and_then(|connection| {
            connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM strategies LIMIT 1)",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .map(|exists| exists != 0)
        .unwrap_or(false)
}

/// Create a distinct on-demand snapshot and apply the same seven-period retention policy.
///
/// Returns:
///     Path to the newly published snapshot directory.
pub fn backup_now() -> anyhow::Result<PathBuf> {
    let src = paths::strategies_db_path();
    let metadata = std::fs::symlink_metadata(&src)
        .map_err(|error| anyhow::Error::from(error).context("БД стратегий ещё не создана"))?;
    anyhow::ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "путь БД стратегий не является обычным файлом"
    );
    let backups = paths::strategies_backups_dir();
    let now_ms = now_unix_ms_i64();
    backup_manual_at(&src, &backups, now_ms)
}

#[cfg(test)]
mod tests;
