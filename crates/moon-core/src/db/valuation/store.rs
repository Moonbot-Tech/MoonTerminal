use super::*;

/// Return whether one SQLite error proves database-page corruption.
///
/// Args:
///     error: SQLite operation failure.
///
/// Returns:
///     Whether SQLite classified the file as corrupt or not a database.
pub(crate) fn is_corruption(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(
                code.code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
            )
    )
}

/// Mark the canonical derived cache unavailable and wake its recovery worker.
///
/// Args:
///     reason: Diagnostic reason retained in the application log.
pub(crate) fn mark_unhealthy(reason: &dyn std::fmt::Display) {
    if CACHE_HEALTH.swap(UNHEALTHY, Ordering::AcqRel) != UNHEALTHY {
        log::warn!("valuation: derived cache disabled: {reason}");
    }
    CACHE_HEALTH_EPOCH.fetch_add(1, Ordering::AcqRel);
    worker::wake_for_recovery();
}

/// Publish a successfully opened canonical cache to new readers.
pub(in crate::db::valuation) fn mark_healthy() {
    CACHE_HEALTH.store(HEALTHY, Ordering::Release);
    CACHE_HEALTH_EPOCH.fetch_add(1, Ordering::AcqRel);
}

/// Disable attachment while startup validation runs without emitting a false damage warning.
pub(in crate::db::valuation) fn begin_store_validation() {
    CACHE_HEALTH.store(UNHEALTHY, Ordering::Release);
    CACHE_HEALTH_EPOCH.fetch_add(1, Ordering::AcqRel);
}

/// Return whether new readers may attach the canonical derived cache.
///
/// Returns:
///     `true` only after startup validation or recovery succeeded.
pub(crate) fn cache_is_healthy() -> bool {
    CACHE_HEALTH.load(Ordering::Acquire) == HEALTHY
}

/// Whether report readers attach the derived cache now, as a revision a caller can compare later.
///
/// A healthy answer is stamped with the health epoch, which every change of health advances: two
/// equal answers mean every reader opened between them attached the cache, not only the readers
/// that happened to open while it looked healthy.
///
/// Returns:
///     The epoch while the cache is attachable; `None` while it is not.
pub fn attach_epoch() -> Option<u64> {
    let before = CACHE_HEALTH_EPOCH.load(Ordering::Acquire);
    let healthy = cache_is_healthy();
    let after = CACHE_HEALTH_EPOCH.load(Ordering::Acquire);
    (healthy && before == after).then_some(before)
}

/// Check one existing cache without creating or mutating it.
///
/// Args:
///     path: Existing valuation database path.
///
/// Returns:
///     `true` for a complete healthy schema, `false` for proven corruption or an invalid schema,
///     and an error description when the check itself could not run conclusively.
fn existing_store_is_healthy(path: &Path) -> Result<bool, String> {
    let uri = sqlite_read_only_uri(path);
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    let conn = Connection::open_with_flags(uri, flags)
        .map_err(|error| format!("read-only open failed: {error}"))?;
    crate::db::trace::install_on(&conn);
    let check = conn.query_row("PRAGMA main.quick_check(1)", [], |row| {
        row.get::<_, String>(0)
    });
    match check {
        Ok(result) if result == "ok" => {}
        Ok(result) => {
            log::warn!("valuation: quick_check reported: {result}");
            return Ok(false);
        }
        Err(error) if is_corruption(&error) => return Ok(false),
        Err(error) => return Err(format!("quick_check failed: {error}")),
    }
    match validate_store_schema(&conn) {
        Ok(()) => Ok(true),
        Err(error) if is_corruption(&error) => Ok(false),
        Err(error) => {
            log::warn!("valuation: existing schema is unusable: {error}");
            Ok(false)
        }
    }
}

/// Validate the two reader-facing tables on a direct valuation connection.
///
/// Args:
///     conn: Direct read-only or read-write valuation connection.
///
/// Returns:
///     SQLite success after both schemas and their first reachable rows are readable.
pub(in crate::db::valuation) fn validate_store_schema(conn: &Connection) -> rusqlite::Result<()> {
    validate_schema_with_prefix(conn, "")
}

/// Prove that the report connection's main schema passes SQLite's bounded consistency check.
///
/// Args:
///     conn: Report reader whose integrity must remain fail-closed.
///
/// Returns:
///     Whether `PRAGMA main.quick_check(1)` returned exactly `ok`.
pub(in crate::db::valuation) fn main_is_healthy(conn: &Connection) -> bool {
    matches!(
        conn.query_row("PRAGMA main.quick_check(1)", [], |row| row.get::<_, String>(0)),
        Ok(result) if result == "ok"
    )
}

/// Prove corruption of a derived file that failed before SQLite could attach its schema.
///
/// Args:
///     conn: Healthy report-main candidate.
///     path: Explicit valuation file passed to `ATTACH`.
///     error: Corruption-class attachment failure.
///
/// Returns:
///     `true` only when report main is healthy and read-only derived validation proves damage.
pub(in crate::db::valuation) fn prove_detached_store_corruption(
    conn: &Connection,
    path: &Path,
    error: &rusqlite::Error,
) -> bool {
    is_corruption(error)
        && main_is_healthy(conn)
        && matches!(existing_store_is_healthy(path), Ok(false))
}

/// Return whether filesystem metadata describes a directory without any link or reparse point.
///
/// Args:
///     metadata: Metadata obtained without following the final path component.
///
/// Returns:
///     `true` only for a plain directory safe to use as a recovery boundary.
fn is_plain_directory(metadata: &std::fs::Metadata) -> bool {
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// Create or validate one recovery directory without following a substituted final component.
///
/// Args:
///     path: Damage root or persistent pending retirement directory.
///
/// Returns:
///     Success only when the resulting path is a plain directory.
fn ensure_plain_directory(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if is_plain_directory(&metadata) => return Ok(()),
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("recovery path is not a plain directory: {}", path.display()),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    std::fs::create_dir_all(path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    if is_plain_directory(&metadata) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("recovery path is not a plain directory: {}", path.display()),
        ))
    }
}

/// Move every remaining live cache member into the persistent pending directory.
///
/// WAL and SHM move before the main database. A crash therefore cannot permit replacement
/// creation while an old main file is still live, and the same pending directory is resumed on
/// the next launch.
///
/// Args:
///     files: Main, WAL, and SHM paths in that order.
///     pending: Stable retirement directory.
///
/// Returns:
///     Filesystem success only after no live cache member remains.
///
/// Errors:
///     Returns an I/O error when the recovery boundary is unsafe or any family member cannot be
///     inspected or moved.
fn retire_live_family(files: &[PathBuf; 3], pending: &Path) -> std::io::Result<()> {
    ensure_plain_directory(pending)?;
    for index in [1usize, 2, 0] {
        let source = &files[index];
        if !source.try_exists()? {
            continue;
        }
        let name = source.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "valuation member has no name",
            )
        })?;
        let destination = pending.join(name);
        if destination.try_exists()? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "pending valuation member already exists: {}",
                    destination.display()
                ),
            ));
        }
        std::fs::rename(source, destination)?;
    }
    for path in files {
        if path.try_exists()? {
            return Err(std::io::Error::other(format!(
                "live valuation member remains: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

/// Publish one completed pending retirement under a unique immutable directory.
///
/// Args:
///     pending: Stable pending directory containing the complete retired family.
///     damage_root: Plain parent directory for the immutable archive.
///
/// Returns:
///     Published quarantine path, or a filesystem failure that leaves `pending` resumable.
///
/// Errors:
///     Returns an I/O error when either boundary is unsafe or the atomic publish rename fails.
fn finalize_pending_retirement(
    pending: &Path,
    damage_root: &Path,
) -> std::io::Result<std::path::PathBuf> {
    ensure_plain_directory(damage_root)?;
    ensure_plain_directory(pending)?;
    let mut timestamp = crate::util::now_unix_ms_i64();
    let process_id = std::process::id();
    loop {
        let archive =
            crate::config::paths::valuation_recovery_archive_in(damage_root, timestamp, process_id);
        if !archive.try_exists()? {
            std::fs::rename(pending, &archive)?;
            return Ok(archive);
        }
        timestamp = timestamp.saturating_add(1);
    }
}

/// Resume or begin crash-consistent retirement of the canonical derived cache.
///
/// Args:
///     files: Main, WAL, and SHM paths in that order.
///     damage_root: Plain parent directory for immutable quarantine archives.
///     pending: Stable directory used to resume an interrupted retirement.
///
/// Returns:
///     Published quarantine directory after every live member is retired.
///
/// Errors:
///     Returns a diagnostic string when directory validation, retirement, or publication fails.
pub(in crate::db::valuation) fn retire_store_family(
    files: &[PathBuf; 3],
    damage_root: &Path,
    pending: &Path,
) -> Result<std::path::PathBuf, String> {
    ensure_plain_directory(damage_root)
        .map_err(|error| format!("create {} failed: {error}", damage_root.display()))?;
    retire_live_family(files, pending)
        .map_err(|error| format!("retire valuation family failed: {error}"))?;
    finalize_pending_retirement(pending, damage_root)
        .map_err(|error| format!("publish valuation retirement failed: {error}"))
}

/// Open one derived store after validating or retiring its complete SQLite family.
///
/// Args:
///     files: Main, WAL, and SHM paths in that order.
///     damage_root: Parent for immutable quarantine directories.
///     pending: Stable directory used to resume interrupted retirement.
///
/// Returns:
///     Healthy read-write cache connection, or a diagnostic failure before replacement.
pub(in crate::db::valuation) fn open_recoverable_store(
    files: [PathBuf; 3],
    damage_root: &Path,
    pending: &Path,
) -> Result<Connection, String> {
    let path = &files[0];
    let pending_exists = pending
        .try_exists()
        .map_err(|error| format!("inspect {} failed: {error}", pending.display()))?;
    let main_exists = path
        .try_exists()
        .map_err(|error| format!("inspect {} failed: {error}", path.display()))?;
    let orphan_exists = files[1..]
        .iter()
        .try_fold(false, |found, member| {
            member.try_exists().map(|exists| found || exists)
        })
        .map_err(|error| format!("inspect valuation sidecar failed: {error}"))?;

    let needs_retirement = if pending_exists || (!main_exists && orphan_exists) {
        true
    } else if main_exists {
        !existing_store_is_healthy(path)?
    } else {
        false
    };
    if needs_retirement {
        let archive = retire_store_family(&files, damage_root, pending)?;
        log::warn!(
            "valuation: damaged derived cache retired to {}",
            archive.display()
        );
    }

    let conn = open_store(path).map_err(|error| format!("initialize fresh store: {error}"))?;
    validate_store_schema(&conn).map_err(|error| format!("validate fresh store: {error}"))?;
    Ok(conn)
}

/// Open the canonical store after validating or retiring every prior live member.
///
/// Existing storage is checked read-only before any journal-mode or schema mutation. A pending
/// retirement always finishes before a replacement may be created.
///
/// Returns:
///     Healthy read-write cache connection, or a diagnostic failure with cache attachment disabled.
pub(crate) fn open_canonical_store() -> Result<Connection, String> {
    let _lifecycle = CACHE_LIFECYCLE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    begin_store_validation();
    let files = crate::config::paths::valuation_db_files();
    let damage_root = crate::config::paths::damaged_valuation_dir();
    let pending = crate::config::paths::valuation_recovery_pending_dir();
    let conn = open_recoverable_store(files, &damage_root, &pending)?;
    mark_healthy();
    Ok(conn)
}

/// Classify a direct cache operation failure and disable attachment on proven corruption.
///
/// Proven corruption is reported as [`FailureKind::CacheUnhealthy`] rather than a write failure
/// because the two need different responses: an ordinary write error is worth retrying against the
/// same file, while a damaged cache is only cleared by the recovery stage rebuilding it.
///
/// Args:
///     error: SQLite failure from the canonical valuation writer.
///
/// Returns:
///     Stage-less classified cause carrying the diagnostic text.
pub(crate) fn store_fault(error: rusqlite::Error) -> FaultCause {
    let kind = if is_corruption(&error) {
        mark_unhealthy(&error);
        FailureKind::CacheUnhealthy
    } else {
        FailureKind::CacheWrite
    };
    FaultCause::new(kind, error.to_string())
}

/// Open and initialize the historical valuation store.
///
/// Args:
///     path: Canonical or fixture SQLite path.
///
/// Returns:
///     WAL-mode connection with the current schema and indexes.
///
/// Errors:
///     Returns the underlying SQLite error when the file or schema cannot be initialized.
pub(crate) fn open_store(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    crate::db::wal::enable(&conn)?;
    // The worker writes one transaction per batch. A less frequent checkpoint reduces checkpoint
    // pressure while the fixed SQLite WAL implementation coordinates attached readers.
    conn.pragma_update(None, "wal_autocheckpoint", 8_192)?;
    // A rebuildable cache: NORMAL keeps WAL atomicity and only risks the last commits on power
    // loss, which the worker re-derives from the report rows.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    // Room for every statement the worker repeats per trade, so none is re-prepared per event.
    conn.set_prepared_statement_cache_capacity(32);
    conn.busy_timeout(Duration::from_secs(3))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS rates (
             algorithm_version INTEGER NOT NULL,
             quote_ordinal INTEGER NOT NULL,
             minute_utc INTEGER NOT NULL,
             resolved_minute_utc INTEGER NOT NULL,
             rate_usdt REAL NOT NULL,
             price_basis INTEGER NOT NULL,
             provider TEXT NOT NULL,
             symbol TEXT NOT NULL,
             orientation INTEGER NOT NULL,
             candle_open_ms INTEGER NOT NULL,
             candle_close_ms INTEGER NOT NULL,
             leg1_rate REAL NOT NULL,
             leg2_provider TEXT,
             leg2_symbol TEXT,
             leg2_orientation INTEGER,
             leg2_rate REAL,
             fetched_at_ms INTEGER NOT NULL,
             PRIMARY KEY (algorithm_version, quote_ordinal, minute_utc)
         );
         CREATE TABLE IF NOT EXISTS rate_searches (
             algorithm_version INTEGER NOT NULL,
             quote_ordinal INTEGER NOT NULL,
             minute_utc INTEGER NOT NULL,
             searched_through_minute INTEGER NOT NULL,
             next_retry_at_ms INTEGER NOT NULL,
             attempts INTEGER NOT NULL,
             updated_at_ms INTEGER NOT NULL,
             PRIMARY KEY (algorithm_version, quote_ordinal, minute_utc)
         );
         CREATE INDEX IF NOT EXISTS idx_rate_searches_retry
             ON rate_searches (algorithm_version, next_retry_at_ms);
         CREATE TABLE IF NOT EXISTS trade_values (
             source_kind INTEGER NOT NULL,
             core_uid INTEGER NOT NULL,
             row_id INTEGER NOT NULL,
             algorithm_version INTEGER NOT NULL,
             closedate INTEGER NOT NULL,
             quote_ordinal INTEGER NOT NULL,
             profit_quote REAL NOT NULL,
             spent_quote REAL,
             rate_minute_utc INTEGER NOT NULL,
             rate_usdt REAL NOT NULL,
             profit_usdt REAL NOT NULL,
             spent_usdt REAL,
             valued_at_ms INTEGER NOT NULL,
             PRIMARY KEY (source_kind, core_uid, row_id)
         );
         CREATE INDEX IF NOT EXISTS idx_trade_values_inputs
             ON trade_values (algorithm_version, quote_ordinal, rate_minute_utc);",
    )?;
    let user_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if user_version < STORE_SCHEMA_VERSION {
        match purge_hyperliquid_collisions(&conn) {
            Ok((values, rates)) if values > 0 || rates > 0 => log::warn!(
                "valuation: purged {values} trade values and {rates} rates priced through a \
                 Hyperliquid ticker collision"
            ),
            Ok(_) => {}
            // user_version stays unset, so the purge retries on the next open.
            Err(err) => {
                log::warn!("valuation: Hyperliquid collision purge failed, retry next open: {err}")
            }
        }
    }
    crate::db::trace::install_on(&conn);
    Ok(conn)
}

/// `PRAGMA user_version` of `valuation.sqlite`.
///
/// Version 1 means the Hyperliquid ticker-collision purge has run on this file.
const STORE_SCHEMA_VERSION: i64 = 1;

/// Build the SQL predicate matching a rate with a Hyperliquid leg outside the allow-list.
///
/// Hyperliquid symbols are persisted as `BASE/QUOTE`; any such leg whose pair is not made of two
/// distinct `resolver::HYPERLIQUID_GENUINE_TICKERS` was produced by a ticker collision.
///
/// Returns:
///     SQL boolean expression over the `rates` columns.
fn hyperliquid_poison_predicate() -> String {
    let tickers = resolver::HYPERLIQUID_GENUINE_TICKERS;
    let list = tickers
        .iter()
        .flat_map(|base| {
            tickers
                .iter()
                .filter(move |quote| *quote != base)
                .map(move |quote| format!("'{base}/{quote}'"))
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "(provider='hyperliquid_spot' AND symbol NOT IN ({list})) \
         OR (leg2_provider='hyperliquid_spot' AND leg2_symbol NOT IN ({list}))"
    )
}

/// Drop cached rates priced through a Hyperliquid ticker collision and their trade values.
///
/// Trade values go first so startup reconciliation revalues exactly those rows through the fixed
/// router. Runs in one transaction and stamps `STORE_SCHEMA_VERSION` on success.
///
/// Args:
///     conn: Open valuation store.
///
/// Returns:
///     Deleted trade-value and rate row counts.
///
/// Errors:
///     Returns the underlying SQLite error; the transaction then rolls back.
fn purge_hyperliquid_collisions(conn: &Connection) -> rusqlite::Result<(usize, usize)> {
    let tx = conn.unchecked_transaction()?;
    let values = tx.execute(
        &format!(
            "DELETE FROM trade_values
             WHERE (algorithm_version, quote_ordinal, rate_minute_utc) IN (
                 SELECT algorithm_version, quote_ordinal, minute_utc FROM rates WHERE {}
             )",
            hyperliquid_poison_predicate()
        ),
        [],
    )?;
    let rates = tx.execute(
        &format!("DELETE FROM rates WHERE {}", hyperliquid_poison_predicate()),
        [],
    )?;
    tx.execute_batch(&format!("PRAGMA user_version = {STORE_SCHEMA_VERSION}"))?;
    tx.commit()?;
    Ok((values, rates))
}
