//! Read-failure classification for the reports replica.
//!
//! Reports-replica reads must distinguish a genuine empty result from a
//! filesystem or SQLite failure. `ReadFail` preserves that distinction from
//! the query origin to the UI.
//!
//! `NotReady` means the replica genuinely has nothing to say yet (file absent,
//! core schema not delivered). `Failed` means filesystem or SQLite access
//! failed; `kind` carries enough detail for failure-specific UI guidance.
//! `IncomparableQuote` is a healthy safety boundary for raw-money scopes whose
//! persisted currency identity is mixed or unknown. `PeriodOutOfRange` is a
//! resolved query period outside the readable range — a corrupt or absurd
//! bound, not an absence of trades, so it must not render as an empty result.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rusqlite::ErrorCode;

use crate::config::paths;

/// Why a read failed, at the granularity the UI actually branches on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FailKind {
    /// This process has no permission to access the replica after lease/recovery preflight.
    ReplicaAccessDenied,
    /// Lock contention past `busy_timeout`; retrying may succeed.
    Busy,
    /// The database image is malformed / not a database. Never self-heals.
    Corrupt,
    /// Filesystem errors, SQLite misuse, and all other read failures.
    Other,
}

/// Numeric origin of a replica read failure.
///
/// SQLite's primary code is the lowest eight bits of the extended code; the pair is
/// what distinguishes `SQLITE_BUSY` from `SQLITE_IOERR_*`. A filesystem refusal
/// carries the OS error number (`5` for Access Denied on Windows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailCode {
    /// SQLite primary result code plus the extended code.
    Sqlite { primary: i32, extended: i32 },
    /// OS error number from a filesystem refusal (`std::io::Error::raw_os_error`).
    Os(i32),
    /// No numeric code: a rusqlite non-`SqliteFailure`, or a constructed outcome.
    None,
}

impl fmt::Display for FailCode {
    /// Render the numeric code as a pasteable diagnostic token.
    ///
    /// Args:
    ///     f: Destination formatter.
    ///
    /// Returns:
    ///     Formatter result after writing `primary/extended`, `os N`, or `-`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            FailCode::Sqlite { primary, extended } => write!(f, "{primary}/{extended}"),
            FailCode::Os(n) => write!(f, "os {n}"),
            FailCode::None => f.write_str("-"),
        }
    }
}

/// Outcome of a replica read that did not produce data.
#[derive(Clone, Debug)]
pub enum ReadFail {
    /// Nothing to read yet: no replica file, or no source carries the columns
    /// required by the caller. The UI presents this as "not synced yet".
    NotReady,
    /// Raw quote money cannot be compared because the query is mixed or unknown.
    IncomparableQuote,
    /// The resolved period lies outside the readable range — a corrupt or absurd bound, not an
    /// absence of trades. Distinct from an empty result so the UI never reads a rejected period
    /// as "nothing happened".
    PeriodOutOfRange,
    /// Filesystem or SQLite access failed. `msg` is the driver's sentence; `path`,
    /// `operation` and `code` travel as typed data so the UI can localize labels
    /// without moon-core formatting a user-facing string.
    Failed {
        kind: FailKind,
        msg: Arc<str>,
        /// Replica (or other) file that the failed operation targeted.
        path: Arc<str>,
        /// Static operation label (`ctx`) that named the query or open.
        operation: &'static str,
        /// SQLite primary+extended codes, or the OS error number.
        code: FailCode,
    },
}

impl ReadFail {
    /// Failure granularity, or `None` for non-database outcomes.
    ///
    /// The UI picks its guidance from this: contention is worth retrying,
    /// corruption requires repair, and `Other` promises neither outcome.
    ///
    /// Returns:
    ///     Database failure kind, or `None` for healthy non-database outcomes.
    pub fn kind(&self) -> Option<FailKind> {
        match self {
            ReadFail::NotReady | ReadFail::IncomparableQuote | ReadFail::PeriodOutOfRange => None,
            ReadFail::Failed { kind, .. } => Some(*kind),
        }
    }

    /// Classified filesystem or SQLite failure carrying replica path, operation and code.
    ///
    /// Args:
    ///     kind: UI-facing granularity.
    ///     msg: Driver sentence already formatted for display.
    ///     path: File the failed operation targeted.
    ///     operation: Static `ctx` label of the query or open.
    ///     code: SQLite primary+extended pair, OS error number, or none.
    ///
    /// Returns:
    ///     `ReadFail::Failed` with every diagnostic field filled.
    pub fn failed(
        kind: FailKind,
        msg: impl Into<Arc<str>>,
        path: impl AsRef<Path>,
        operation: &'static str,
        code: FailCode,
    ) -> Self {
        ReadFail::Failed {
            kind,
            msg: msg.into(),
            path: path_text(path.as_ref()),
            operation,
            code,
        }
    }

    /// Replica file path carried by a database failure.
    ///
    /// Returns:
    ///     Display path, or `None` for non-database outcomes.
    pub fn path(&self) -> Option<&str> {
        match self {
            ReadFail::Failed { path, .. } => Some(path.as_ref()),
            _ => None,
        }
    }

    /// Operation label (`ctx`) carried by a database failure.
    ///
    /// Returns:
    ///     Static operation string, or `None` for non-database outcomes.
    pub fn operation(&self) -> Option<&'static str> {
        match self {
            ReadFail::Failed { operation, .. } => Some(*operation),
            _ => None,
        }
    }

    /// Numeric code carried by a database failure.
    ///
    /// Returns:
    ///     SQLite or OS code, or `None` for non-database outcomes.
    pub fn code(&self) -> Option<FailCode> {
        match self {
            ReadFail::Failed { code, .. } => Some(*code),
            _ => None,
        }
    }
}

impl std::fmt::Display for ReadFail {
    /// Render the originating failure message or non-database outcome text.
    ///
    /// Args:
    ///     f: Destination formatter.
    ///
    /// Returns:
    ///     Formatter result after writing the stable diagnostic text.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadFail::NotReady => f.write_str("replica not ready"),
            ReadFail::IncomparableQuote => f.write_str("quote currency is mixed or unknown"),
            ReadFail::PeriodOutOfRange => f.write_str("period outside the readable range"),
            ReadFail::Failed { msg, .. } => f.write_str(msg),
        }
    }
}

/// Result of any fallible replica read.
pub type ReadResult<T> = Result<T, ReadFail>;

/// Repeats of the same `(ctx, kind)` are collapsed within this window.
///
/// Without it a repeating failure floods both sinks: the Report panel requeries
/// whenever the writer generation bumps (seconds apart) and each pass runs
/// several probes, so one damaged replica could evict every other line from the
/// Log panel's 5000-line ring and append to the dated file without pause.
const WARN_REPEAT_GAP: Duration = Duration::from_secs(60);

/// Last log time + suppressed count per `(ctx, kind)`. `None` — never logged
/// yet (an `Instant` has no representable "long ago", so the absence is the
/// first-time marker rather than a back-dated timestamp).
///
/// The key borrows `ctx` instead of formatting it: this map is consulted on
/// every failure INCLUDING the suppressed repeats the throttle exists to make
/// cheap, and a damaged replica re-queried on each writer generation would
/// otherwise pay an allocation per event it is trying not to spend anything on.
static WARN_SEEN: OnceLock<Mutex<HashMap<(&'static str, FailKind), (Option<Instant>, u64)>>> =
    OnceLock::new();

/// Classify a SQLite error and log it at the origin, so the Log panel and the
/// dated log file carry the line even when the Analytics window is closed.
/// `ctx` names the query that failed (e.g. `"analytics: scan_period prepare"`).
///
/// Corruption also latches the shared integrity failure so the sole writer stops
/// before another batch and the Analytics warning reflects damage found after
/// the one-shot background scan.
///
/// Repeats of the same `(ctx, kind)` are collapsed — see [`WARN_REPEAT_GAP`].
///
/// Args:
///     ctx: Static operation label for diagnostics and throttling.
///     error: SQLite failure to classify and publish.
///
/// Returns:
///     Classified failure suitable for propagation to the UI.
pub(crate) fn read_fail(ctx: &'static str, error: rusqlite::Error) -> ReadFail {
    read_fail_at(ctx, &paths::reports_db_path(), error)
}

/// Classify a SQLite error against a known file path.
///
/// Same as [`read_fail`], for opens that target a file other than the reports replica
/// (the strategy database).
///
/// Args:
///     ctx: Static operation label for diagnostics and throttling.
///     path: File the failed statement or open targeted.
///     error: SQLite failure to classify and publish.
///
/// Returns:
///     Classified failure suitable for propagation to the UI.
pub(crate) fn read_fail_at(ctx: &'static str, path: &Path, error: rusqlite::Error) -> ReadFail {
    if let Some(cancelled) = cancelled_read(&error) {
        return cancelled;
    }
    let kind = classify(&error);
    if kind == FailKind::Corrupt {
        super::integrity::record_corruption(&error);
    }
    let code = sqlite_code(&error);
    log_throttled(ctx, kind, &error, path.to_string_lossy().as_ref(), code);
    ReadFail::failed(kind, format!("{error}"), path, ctx, code)
}

/// Classify a report-reader failure while isolating proven attached-cache corruption.
///
/// A `SQLITE_CORRUPT` code does not name the damaged schema. This boundary suppresses the reports
/// latch only after independent checks prove `main` healthy and `valuation` damaged. Any
/// inconclusive result delegates to [`read_fail`] and remains fail-closed.
///
/// Args:
///     conn: Report reader that executed the failing statement.
///     ctx: Static operation label for diagnostics and throttling.
///     error: SQLite failure to classify.
///
/// Returns:
///     Ordinary report failure, or a non-corruption failure while the derived cache rebuilds.
pub(crate) fn read_fail_on(
    conn: &rusqlite::Connection,
    ctx: &'static str,
    error: rusqlite::Error,
) -> ReadFail {
    if let Some(cancelled) = cancelled_read(&error) {
        return cancelled;
    }
    if super::valuation::prove_derived_corruption(conn, &error) {
        let path = paths::reports_db_path();
        let code = sqlite_code(&error);
        log_throttled(
            ctx,
            FailKind::Other,
            &error,
            path.to_string_lossy().as_ref(),
            code,
        );
        return ReadFail::failed(
            FailKind::Other,
            format!("historical valuation cache is rebuilding: {error}"),
            &path,
            ctx,
            code,
        );
    }
    read_fail(ctx, error)
}

/// Recognize the expected SQLite interrupt of an explicitly superseded read.
///
/// The request generation still prevents publication; this classification keeps the discarded
/// completion from logging a database fault or probing attached-cache corruption.
///
/// Args:
///     error: SQLite failure produced by the interrupted statement.
///
/// Returns:
///     Silent ordinary failure for a requested interrupt, or `None` for every real fault.
fn cancelled_read(error: &rusqlite::Error) -> Option<ReadFail> {
    let interrupted = matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == ErrorCode::OperationInterrupted
    );
    (interrupted && super::read_cancel::take_requested_interrupt()).then(|| {
        ReadFail::failed(
            FailKind::Other,
            "database read superseded",
            paths::reports_db_path(),
            "read superseded",
            sqlite_code(error),
        )
    })
}

/// Map a SQLite error onto the granularity the UI branches on.
///
/// NOTE: corruption reaching us as an extended I/O code (`SQLITE_IOERR_*`)
/// lands in `Other`, not `Corrupt` — it is still a hard failure and still
/// surfaces, it just does not get the permanent-failure hint.
pub(super) fn classify(e: &rusqlite::Error) -> FailKind {
    match e {
        rusqlite::Error::SqliteFailure(err, _) => match err.code {
            ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => FailKind::Corrupt,
            ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => FailKind::Busy,
            _ => FailKind::Other,
        },
        _ => FailKind::Other,
    }
}

/// Wrap a filesystem error that stopped a read before SQLite was reached.
///
/// An unreadable existing replica is a failure; only a genuinely absent file
/// qualifies as `NotReady`.
pub(super) fn io_fail(ctx: &'static str, path: &Path, e: &std::io::Error) -> ReadFail {
    let code = os_code(e);
    log_throttled(
        ctx,
        FailKind::Other,
        e,
        path.to_string_lossy().as_ref(),
        code,
    );
    ReadFail::failed(FailKind::Other, format!("{e}"), path, ctx, code)
}

/// SQLite primary (low 8 bits) and extended result codes, or `None` when rusqlite
/// did not wrap a `SqliteFailure`.
fn sqlite_code(error: &rusqlite::Error) -> FailCode {
    match error {
        rusqlite::Error::SqliteFailure(err, _) => FailCode::Sqlite {
            primary: err.extended_code & 0xFF,
            extended: err.extended_code,
        },
        _ => FailCode::None,
    }
}

/// OS error number from a filesystem refusal, or `None` when the platform did not
/// supply one.
fn os_code(e: &std::io::Error) -> FailCode {
    match e.raw_os_error() {
        Some(n) => FailCode::Os(n),
        None => FailCode::None,
    }
}

/// Display form of a replica path stored on `ReadFail`.
fn path_text(path: &Path) -> Arc<str> {
    Arc::from(path.to_string_lossy().as_ref())
}

/// One throttled warning line: operation, driver sentence, path and numeric code.
///
/// Args:
///     ctx: Static operation label.
///     e: Driver error text.
///     path: File the failed operation targeted.
///     code: SQLite or OS code.
///     suppressed: Repeat count collapsed inside the throttle window; `0` omits it.
///
/// Returns:
///     ASCII diagnostic line written to the log sink.
fn warn_line(
    ctx: &str,
    e: &dyn fmt::Display,
    path: &str,
    code: FailCode,
    suppressed: u64,
) -> String {
    if suppressed > 0 {
        format!("{ctx}: {e} path={path} code={code} (повторов подавлено: {suppressed})")
    } else {
        format!("{ctx}: {e} path={path} code={code}")
    }
}

/// Log the first failure immediately and summarize repeats after the throttle window.
fn log_throttled(
    ctx: &'static str,
    kind: FailKind,
    e: &dyn fmt::Display,
    path: &str,
    code: FailCode,
) {
    let map = WARN_SEEN.get_or_init(|| Mutex::new(HashMap::new()));
    let mut seen = match map.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let now = Instant::now();
    let (last, suppressed) = seen.entry((ctx, kind)).or_insert((None, 0));
    if last.is_some_and(|t| now.duration_since(t) < WARN_REPEAT_GAP) {
        *suppressed += 1;
        return;
    }
    *last = Some(now);
    let n = std::mem::replace(suppressed, 0);
    drop(seen); // do not hold the global lock while the log sink runs
    log::warn!("{}", warn_line(ctx, e, path, code, n));
}

#[cfg(test)]
/// Classification and warning-throttle regression tests.
mod tests;
