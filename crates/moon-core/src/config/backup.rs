//! Daily snapshots of the irreplaceable settings pair under `backups/settings/`.
//!
//! `servers.enc` and `cfg/settings.toml` form one logical generation, so config writes and backup
//! copies share [`CONFIG_PAIR_LOCK`]. Routine and deliberate saves never create snapshots; the
//! unified daily scheduler owns the 12:00 UTC slot. A schema migration uses that same slot before
//! overwriting older bytes, preserving the migration safety barrier without creating an extra copy.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use crate::backup_store::{ExactPublication, SnapshotStore, timestamp_prefix};
use crate::backups::{DAY_MS, DueOutcome, RETAIN_PERIODS, due_slot_ms};
use crate::util::{now_unix_ms_i64, utc_stamp_compact};

use super::paths;

/// Marker written after every new settings snapshot finishes copying both required sources.
const COMPLETION_NAME: &str = ".complete";
/// Versioned contents distinguish completed settings snapshots from unrelated directories.
const COMPLETION_CONTENT: &[u8] = b"moonterminal-settings-backup-v1\n";
/// Serializes the complete config pair across writes and snapshot reads.
static CONFIG_PAIR_LOCK: Mutex<()> = Mutex::new(());

/// Lock the settings pair while recovering poison so one panic does not disable future saves.
fn lock_config_pair() -> MutexGuard<'static, ()> {
    CONFIG_PAIR_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

/// Execute one operation while the two config files remain in the same logical generation.
pub(super) fn with_config_pair<T>(operation: impl FnOnce() -> T) -> T {
    let _guard = lock_config_pair();
    operation()
}

/// Return whether an interrupted config-pair replacement still needs reconciliation.
pub(super) fn pair_write_pending() -> bool {
    match std::fs::symlink_metadata(paths::config_pair_pending_path()) {
        Ok(_) => true,
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

/// Mark the config pair uncertain before replacing either member.
pub(super) fn begin_pair_write() -> anyhow::Result<()> {
    let marker = paths::config_pair_pending_path();
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(marker, b"moonterminal-config-pair-v1\n")?;
    Ok(())
}

/// Clear the uncertainty marker only after both config replacements succeed.
pub(super) fn finish_pair_write() -> anyhow::Result<()> {
    match std::fs::remove_file(paths::config_pair_pending_path()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Return whether a directory contains only owned files and every expected settings source.
fn is_completed_snapshot(directory: &Path, source_names: &[&str]) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(directory) else {
        return false;
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    let mut sources = 0usize;
    let mut marker_seen = false;
    for entry in entries {
        let Ok(entry) = entry else {
            return false;
        };
        let Ok(kind) = entry.file_type() else {
            return false;
        };
        if !kind.is_file() || kind.is_symlink() {
            return false;
        }
        let name = entry.file_name();
        if name == OsStr::new(COMPLETION_NAME) {
            marker_seen = std::fs::read(entry.path())
                .map(|bytes| bytes == COMPLETION_CONTENT)
                .unwrap_or(false);
            if !marker_seen {
                return false;
            }
        } else if source_names
            .iter()
            .any(|expected| name == OsStr::new(expected))
        {
            sources += 1;
        } else {
            return false;
        }
    }
    marker_seen && sources == source_names.len()
}

/// Return whether a name belongs to a settings snapshot, including legacy collision suffixes.
fn is_snapshot_name(name: &str) -> bool {
    timestamp_prefix(name).is_some()
        && match name.len() {
            15 => true,
            18 => {
                name.get(15..16) == Some("-")
                    && name
                        .get(16..)
                        .is_some_and(|suffix| suffix.bytes().all(|byte| byte.is_ascii_digit()))
            }
            _ => false,
        }
}

/// Remove completed settings snapshots older than the latest seven UTC-noon periods.
fn prune(backups: &Path, current_slot_ms: i64, source_names: &[&str], expected: &[&str]) -> usize {
    let cutoff = utc_stamp_compact(current_slot_ms - (RETAIN_PERIODS - 1) * DAY_MS);
    let removed = SnapshotStore::new(backups, expected, false).prune_where(
        is_snapshot_name,
        |name| timestamp_prefix(name).is_some_and(|stamp| stamp < cutoff.as_str()),
        |directory| is_completed_snapshot(directory, source_names),
    );
    if removed > 0 {
        log::info!("settings backups: pruned {removed} snapshots older than seven UTC periods");
    }
    removed
}

/// Build and atomically publish the latest due settings slot on injected paths.
fn backup_due_into(sources: &[&Path], backups: &Path, now_ms: i64) -> anyhow::Result<DueOutcome> {
    let source_names: Vec<&str> = sources
        .iter()
        .filter_map(|source| source.file_name()?.to_str())
        .collect();
    anyhow::ensure!(
        source_names.len() == sources.len(),
        "settings backup source has no UTF-8 filename"
    );
    let mut expected = source_names.clone();
    expected.push(COMPLETION_NAME);
    let store = SnapshotStore::new(backups, &expected, false);
    let slot = due_slot_ms(now_ms);
    let stamp = utc_stamp_compact(slot);
    let destination = backups.join(&stamp);
    if is_completed_snapshot(&destination, &source_names) {
        prune(backups, slot, &source_names, &expected);
        return Ok(DueOutcome::Current(destination));
    }
    match std::fs::symlink_metadata(&destination) {
        Ok(_) => anyhow::bail!(
            "scheduled settings backup path is occupied by an incomplete or foreign entry: {}",
            destination.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let mut captured = Vec::new();
    for source in sources {
        match std::fs::symlink_metadata(source) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                let name = source
                    .file_name()
                    .ok_or_else(|| anyhow::anyhow!("settings backup source has no filename"))?
                    .to_owned();
                captured.push((*source, name, std::fs::read(source)?));
            }
            Ok(_) => anyhow::bail!(
                "settings backup source is not a regular file: {}",
                source.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if captured.len() != sources.len() {
        return Ok(DueOutcome::SourceMissing);
    }

    let staging = store.create_staging()?;
    let assembled = (|| -> anyhow::Result<()> {
        for (_, name, bytes) in &captured {
            std::fs::write(staging.join(name), bytes)?;
        }
        // The process mutex covers our own saves. Re-reading both sources also rejects a mixed
        // generation if another MoonTerminal process replaced either file during capture.
        for (source, _, bytes) in &captured {
            let metadata = std::fs::symlink_metadata(source)?;
            anyhow::ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "settings backup source changed type during capture: {}",
                source.display()
            );
            anyhow::ensure!(
                std::fs::read(source)? == *bytes,
                "settings backup source changed during capture: {}",
                source.display()
            );
        }
        std::fs::write(staging.join(COMPLETION_NAME), COMPLETION_CONTENT)?;
        anyhow::ensure!(
            is_completed_snapshot(&staging, &source_names),
            "staged settings backup failed completion validation"
        );
        Ok(())
    })();
    if let Err(error) = assembled {
        store.discard_staging(&staging);
        return Err(error);
    }
    let published = store.publish_exact(&staging, &stamp, |directory| {
        is_completed_snapshot(directory, &source_names)
    })?;
    prune(backups, slot, &source_names, &expected);
    Ok(match published {
        ExactPublication::Created(path) => DueOutcome::Created(path),
        ExactPublication::Existing(path) => DueOutcome::Current(path),
    })
}

/// Back up the latest due settings slot while the config pair cannot change between copies.
pub(super) fn backup_due_at(now_ms: i64) -> anyhow::Result<DueOutcome> {
    with_config_pair(|| {
        anyhow::ensure!(
            !pair_write_pending(),
            "settings config pair has an unfinished replacement"
        );
        let sources = [paths::servers_path(), paths::settings_path()];
        let refs: Vec<&Path> = sources.iter().map(PathBuf::as_path).collect();
        backup_due_into(&refs, &paths::settings_backups_dir(), now_ms)
    })
}

/// Preserve pre-migration config bytes in the same canonical daily slot used by the scheduler.
pub(super) fn backup_before_schema_migration() {
    match backup_due_at(now_unix_ms_i64()) {
        Ok(DueOutcome::Created(path)) => {
            log::info!(
                "settings backup created before schema migration: {}",
                path.display()
            )
        }
        Ok(DueOutcome::Current(_)) | Ok(DueOutcome::SourceMissing) => {}
        Ok(DueOutcome::Pending) => unreachable!("settings backups have no readiness predicate"),
        Err(error) => log::warn!("settings backup before schema migration failed: {error:#}"),
    }
}

#[cfg(test)]
mod tests;
