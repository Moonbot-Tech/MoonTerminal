//! SQLite file maintenance for the Storage tab: compaction and backup.
//! Uses a SEPARATE connection while writers keep their own connections: WAL permits
//! this, and exclusive VACUUM access is acquired between writer batches (`busy_timeout` waits).

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

/// VACUUM to reclaim deleted-row space, then checkpoint (TRUNCATE) the frames it wrote so the
/// `-wal` file does not stay the size of the database ([`super::wal::vacuum`]). Blocks writers
/// during VACUUM and therefore runs only after an explicit user action.
///
/// Args:
///     path: Database to compact; the reports path additionally requires the process lease.
///
/// Returns:
///     Success after VACUUM completes.
pub fn compact_db(path: &Path) -> anyhow::Result<()> {
    if path == crate::config::paths::reports_db_path() {
        super::report_recovery::ensure_access()?;
    }
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
