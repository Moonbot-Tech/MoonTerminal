//! SQLite file maintenance for the Storage tab: compaction and backup.
//! Tape compaction uses its writer queue; other databases use a maintenance connection.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;

/// VACUUM to reclaim deleted-row space, then checkpoint (TRUNCATE) the frames it wrote so the
/// `-wal` file does not stay the size of the database ([`super::wal::vacuum`]). Blocks writers
/// during VACUUM; the daily coordinator acquires any required process lease first.
///
/// Args:
///     path: Database to compact; the reports path additionally requires the process lease.
///
/// Returns:
///     Success after VACUUM completes.
fn compact_db(path: &Path) -> anyhow::Result<()> {
    let conn = Connection::open(path)?;
    let _ = conn.busy_timeout(Duration::from_secs(30));
    super::wal::vacuum(&conn)?;
    Ok(())
}

/// Create a consistent single-file backup (`VACUUM INTO`) from a live database
/// without stopping the writer. The destination file must not exist.
pub fn backup_db(src: &Path, dst: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        !dst.exists(),
        "файл бэкапа уже существует: {}",
        dst.display()
    );
    let conn = Connection::open(src)?;
    let _ = conn.busy_timeout(Duration::from_secs(30));
    let dst_sql = dst.to_string_lossy().replace('\'', "''");
    conn.execute(&format!("VACUUM INTO '{dst_sql}'"), [])?;
    Ok(())
}

/// Return the database and `-wal` file sizes in bytes; missing files count as zero.
pub fn db_sizes(path: &Path) -> (u64, u64) {
    let main = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    let wal = std::fs::metadata(std::path::PathBuf::from(wal))
        .map(|m| m.len())
        .unwrap_or(0);
    (main, wal)
}

/// Compact the due daily slot without overlapping a strategy snapshot.
pub(crate) fn compact_due_at(now_ms: i64) -> anyhow::Result<crate::backups::DueOutcome> {
    compact_due_into(
        &crate::config::paths::compaction_marker_path(),
        &[
            crate::config::paths::reports_db_path(),
            crate::config::paths::strategies_db_path(),
            crate::config::paths::trades_db_path(),
        ],
        now_ms,
        crate::disk_reserve::vacuum_has_room,
        |db| {
            if db == crate::config::paths::trades_db_path() {
                return compact_daily_trades(crate::market::trade_replay::trade_cache::handle());
            }
            compact_daily_db(
                db,
                &crate::config::paths::reports_db_path(),
                &crate::config::paths::strategies_db_path(),
                &crate::backups::STRATEGY_MAINTENANCE,
                super::report_recovery::wait_for_write_access,
                compact_db,
            )
        },
    )
}

/// Skip an absent worker or insufficient room, and retry genuine worker failures later.
fn compact_daily_trades(
    cache: Option<crate::market::trade_replay::trade_cache::TradeCache>,
) -> anyhow::Result<()> {
    let Some(cache) = cache else {
        log::warn!("daily trades compaction skipped: no tape worker");
        return Ok(());
    };
    compact_daily_trades_result(cache.compact_full())
}

/// A running VACUUM owns this daily slot even after the caller stops waiting.
fn compact_daily_trades_result(
    result: Result<
        Result<(), crate::market::trade_replay::trade_cache::CompactFail>,
        std::sync::mpsc::RecvTimeoutError,
    >,
) -> anyhow::Result<()> {
    use crate::market::trade_replay::trade_cache::CompactFail;
    match result {
        Ok(Ok(())) => Ok(()),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            log::warn!("daily trades compaction still running on tape worker after timeout");
            Ok(())
        }
        Ok(Err(CompactFail::NoRoom)) => {
            log::warn!("daily trades compaction skipped: insufficient disk room");
            Ok(())
        }
        result => anyhow::bail!("daily trades compaction failed: {result:?}"),
    }
}

/// Skip a refused reports lease and exclude snapshots only while compacting strategies.
fn compact_daily_db(
    db: &Path,
    reports: &Path,
    strategies: &Path,
    maintenance: &std::sync::Mutex<()>,
    reports_access: impl FnOnce() -> anyhow::Result<()>,
    compact: impl FnOnce(&Path) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if db == reports {
        if let Err(error) = reports_access() {
            log::warn!("daily reports compaction skipped: {error:#}");
            return Ok(());
        }
    }
    let _guard = if db == strategies {
        let guard = maintenance
            .try_lock()
            .map_err(|_| anyhow::anyhow!("strategy maintenance is active"))?;
        anyhow::ensure!(
            !crate::backups::strategy_job_active(),
            "strategy backup is active"
        );
        Some(guard)
    } else {
        None
    };
    compact(db)
}

/// Persist per-database progress so a deferred strategy backup cannot starve peer maintenance.
fn compact_due_into(
    marker: &Path,
    dbs: &[PathBuf],
    now_ms: i64,
    room: impl Fn(&Path) -> bool,
    compact: impl Fn(&Path) -> anyhow::Result<()>,
) -> anyhow::Result<crate::backups::DueOutcome> {
    use crate::backups::{DueOutcome, due_slot_ms};
    let slot = due_slot_ms(now_ms);
    let mut done = std::collections::BTreeSet::<usize>::new();
    match std::fs::read_to_string(marker) {
        Ok(text) if text.trim().parse::<i64>().is_ok_and(|last| last >= slot) => {
            return Ok(DueOutcome::Current(marker.to_path_buf()));
        }
        Ok(text) => {
            if let Some((previous, entries)) = text.trim().split_once(':') {
                if previous.parse::<i64>().ok() == Some(slot) {
                    done.extend(
                        entries
                            .split(',')
                            .filter_map(|entry| entry.parse::<usize>().ok()),
                    );
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut failed = None;
    for (index, db) in dbs.iter().enumerate() {
        if done.contains(&index) {
            continue;
        }
        if !db.exists() {
            done.insert(index);
            continue;
        }
        if room(db) {
            if let Err(error) = compact(db) {
                log::warn!("daily compaction deferred for {}: {error:#}", db.display());
                failed = Some(error);
                continue;
            }
        } else {
            log::warn!(
                "daily compaction skipped for insufficient disk room: {}",
                db.display()
            );
        }
        done.insert(index);
        let entries = done
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(",");
        std::fs::write(marker, format!("{slot}:{entries}"))?;
    }
    if let Some(error) = failed {
        return Err(error);
    }
    std::fs::write(marker, slot.to_string())?;
    Ok(DueOutcome::Created(marker.to_path_buf()))
}

#[cfg(test)]
mod tests;
