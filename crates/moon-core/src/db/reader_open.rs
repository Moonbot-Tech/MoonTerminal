use super::*;

// ============================================================================
//  Reads for the Reports window
// ============================================================================

/// Map a database path to a read outcome BEFORE opening it: a genuinely absent file becomes
/// `NotReady`, while any other metadata error becomes `Failed`.
///
/// NOT `Path::exists`: it reports false for a permission or metadata error too, which would
/// take the silent `NotReady` branch and tell the user the replica is merely not synced yet —
/// only a genuine absence may say that. The check still has to happen because
/// `Connection::open` would otherwise CREATE an empty database in place of the missing one.
/// `ctx` names the caller for the failure log. Shared by every path that opens a replica or
/// the strategy database read-only (the callers are this module and its descendants, so a
/// plain-private item reaches all of them without widening visibility to the crate root).
pub(in crate::db) fn metadata_gate(path: &std::path::Path, ctx: &'static str) -> ReadResult<()> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ReadFail::NotReady),
        Err(e) => Err(read_fail::io_fail(ctx, path, &e)),
    }
}

/// Main report-schema page-cache budget for one reader connection, as SQLite's
/// negative `cache_size` form (KiB rather than pages, so it does not move with
/// `page_size`).
///
/// The default is ~2 MiB against a replica of hundreds of MB, so a reader starts
/// evicting pages inside a long period scan. Analytics compound reads also perform
/// quote preflight, comparison, and optional lens-neutral work on the same snapshot,
/// so retaining their recently visited index and table pages still matters.
///
/// The budget is per connection. Concurrent readers are capped at
/// [`reader_budget::READER_BUDGET`], so peak page-cache memory is a hard
/// ceiling of 8 × 16 MiB; raising this constant raises that ceiling
/// proportionally.
pub(in crate::db) const READER_CACHE_KIB: i64 = -16_384;

/// Apply the shared tuning of a read-only report connection.
///
/// Both settings are advisory: a failure changes how fast the connection is, never
/// what it returns, so neither is allowed to fail opening a reader.
///
/// Intentionally omitted:
/// - `temp_store = MEMORY` would keep the sorter's temporary b-tree off disk, but
///   a spill to disk still beats an out-of-memory kill even with readers now
///   bounded — a behaviour change, which is exactly what this must not be.
/// - `mmap_size` would serve pages through a memory mapping, where truncation can
///   surface as an OS-level fault instead of an `SQLITE_IOERR`/`SQLITE_CORRUPT`
///   that [`read_fail`] can classify and present with repair guidance.
///
/// Args:
///     conn: Freshly opened read-only connection to the report replica.
pub(in crate::db) fn tune_reader(conn: &Connection) {
    let _ = conn.busy_timeout(Duration::from_secs(3));
    let _ = conn.pragma_update(None, "cache_size", READER_CACHE_KIB);
}

/// Companion databases a report reader attaches.
///
/// A query that reaches `valuation::projection`, `valuation::is_attached`, or
/// `with_valuation_fallback` MUST be opened with valuation attached, because
/// those degrade silently to native money when the schema is absent rather than
/// failing. Construct only through the named constants; there is no public
/// constructor and no `MAIN_ONLY` combination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttachSet {
    /// Attach the strategies database as `strat`.
    strategies: bool,
    /// Attach the valuation cache.
    valuation: bool,
}

impl AttachSet {
    /// Strategies and valuation — the default for Analytics and Report queries.
    pub const ALL: AttachSet = AttachSet {
        strategies: true,
        valuation: true,
    };

    /// Strategies only: chart trade history never references the valuation schema.
    pub const STRATEGIES_ONLY: AttachSet = AttachSet {
        strategies: true,
        valuation: false,
    };

    /// Whether this set attaches the strategies database.
    pub(crate) fn strategies(self) -> bool {
        self.strategies
    }

    /// Whether this set attaches the valuation cache.
    pub(crate) fn valuation(self) -> bool {
        self.valuation
    }
}

/// Attach the companion databases this query asked for.
///
/// ATTACH cannot run inside a transaction, so this runs before any snapshot.
///
/// Args:
///     conn: Freshly opened report connection.
///     attach: Companion databases this query needs.
///
/// Returns:
///     `Ok(())` after the requested attaches, or a fail-closed valuation error.
pub(in crate::db) fn attach_databases(conn: &Connection, attach: AttachSet) -> ReadResult<()> {
    if attach.strategies() {
        // The strategy database rides along on EVERY reader.
        //
        // `unified_from` is shared by Analytics readers, so attaching where the connection is born
        // keeps strategy-aware filtering consistent across all of them. It must happen before any
        // transaction is opened because SQLite does not allow ATTACH inside a transaction.
        analytics::attach_strategies(conn);
    }
    if attach.valuation() {
        // Historical valuations are derived but correctness-sensitive: an existing unreadable cache
        // is a read failure, never silently interpreted as zero coverage. Startup initializes the file
        // before report-derived views can open readers; an absent file remains a normal not-ready state.
        let _ = valuation::attach(conn)?;
    }
    Ok(())
}

/// Open a reader with a chosen companion-database attach set.
///
/// Same readiness and access rules as [`open_reader`]. The attach set is a
/// property of the query: valuation-touching reads must use [`AttachSet::ALL`].
///
/// Args:
///     attach: Companion databases this query needs.
///
/// Returns:
///     Tuned report connection whose descriptors are held under the reader budget.
pub fn open_reader_with(attach: AttachSet) -> ReadResult<ReportReader> {
    let path = paths::reports_db_path();
    report_recovery::ensure_access().map_err(|error| {
        ReadFail::failed(
            FailKind::ReplicaAccessDenied,
            error.to_string(),
            &path,
            "reports: access",
            FailCode::None,
        )
    })?;
    metadata_gate(&path, "отчёты(reader): доступ к файлу")?;
    if read_cancel::current_is_cancelled() {
        return Err(read_fail::superseded_read(FailCode::None));
    }
    let permit = match reader_budget::acquire_with_cancellation(reader_budget::READER_WAIT, &|| {
        read_cancel::current_is_cancelled()
    }) {
        reader_budget::AcquireOutcome::Permit(permit) => permit,
        reader_budget::AcquireOutcome::Cancelled => {
            return Err(read_fail::superseded_read(FailCode::None));
        }
        reader_budget::AcquireOutcome::Timeout => {
            return Err(read_fail::budget_fail("reports: reader budget"));
        }
    };
    let conn =
        Connection::open(&path).map_err(|e| read_fail::read_fail_at("отчёты(reader)", &path, e))?;
    // Before the ATTACH below: `cache_size` applies to one schema, and `main` is the
    // one every period scan reads.
    tune_reader(&conn);
    attach_databases(&conn, attach)?;
    read_cancel::install_current(&conn)?;
    trace::install_on(&conn);
    Ok(ReportReader {
        conn,
        _permit: permit,
    })
}

/// Open a reader while distinguishing an absent replica from a SQLite failure.
///
/// A genuinely absent file maps to `NotReady`; metadata and SQLite open errors
/// map to `Failed` so callers cannot present them as an empty period. Access also
/// fails with a typed denial when the lease/recovery preflight did not authorize access.
///
/// Returns:
///     Tuned report connection for the sole current process.
pub fn open_reader() -> ReadResult<ReportReader> {
    open_reader_with(AttachSet::ALL)
}

/// Pin one WAL snapshot for a multi-statement read.
///
/// Separate autocommit statements each observe a newer snapshot, so a panel
/// could publish a row list and totals that disagree about which trades fall
/// inside the period. Read-only: dropping the transaction rolls back nothing.
/// ATTACH cannot run inside a transaction, so callers attach first. Transaction
/// creation errors map to `Failed`; this function cannot produce `NotReady`.
pub fn read_snapshot(conn: &Connection) -> ReadResult<rusqlite::Transaction<'_>> {
    conn.unchecked_transaction()
        .map_err(|e| read_fail("отчёты: снимок для чтения", e))
}

/// Run a multi-query read inside one pinned SQLite snapshot and one pinned current-rate snapshot.
///
/// Args:
///     conn: Reader connection with every required database already attached.
///     read: Query batch that must observe one committed report generation and one rate snapshot.
///
/// Returns:
///     The batch result, or a classified snapshot/query failure.
pub(in crate::db) fn with_read_snapshot<T>(
    conn: &Connection,
    read: impl FnOnce(&Connection) -> ReadResult<T>,
) -> ReadResult<T> {
    // The rate snapshot is pinned alongside the row snapshot, for the same reason: everything one
    // batch shows the user must describe ONE state of the world. Without it a worker publication
    // landing mid-batch would convert the preflight and the scan at different rates.
    let _rates = valuation::pin_current_rates();
    let snapshot = read_snapshot(conn)?;
    read(&snapshot)
}
