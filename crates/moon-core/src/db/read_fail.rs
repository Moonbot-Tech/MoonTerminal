//! Read-failure classification for the reports replica.
//!
//! Reports-replica reads must distinguish a genuine empty result from a
//! filesystem or SQLite failure. `ReadFail` preserves that distinction from
//! the query origin to the UI.
//!
//! `NotReady` means the replica genuinely has nothing to say yet (file absent,
//! core schema not delivered). `Failed` means filesystem or SQLite access
//! failed; `kind` carries enough detail for failure-specific UI guidance.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rusqlite::ErrorCode;

/// Why a read failed, at the granularity the UI actually branches on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FailKind {
    /// Lock contention past `busy_timeout`; retrying may succeed.
    Busy,
    /// The database image is malformed / not a database. Never self-heals.
    Corrupt,
    /// Filesystem errors, SQLite misuse, and all other read failures.
    Other,
}

/// Outcome of a replica read that did not produce data.
#[derive(Clone, Debug)]
pub enum ReadFail {
    /// Nothing to read yet: no replica file, or no source carries the columns
    /// required by the caller. The UI presents this as "not synced yet".
    NotReady,
    /// Filesystem or SQLite access failed. `msg` is pre-formatted for display;
    /// the origin already logged the failure.
    Failed { kind: FailKind, msg: Arc<str> },
}

impl ReadFail {
    /// Failure granularity, or `None` for the not-ready sentinel.
    ///
    /// The UI picks its guidance from this: contention is worth retrying,
    /// corruption requires repair, and `Other` promises neither outcome.
    pub fn kind(&self) -> Option<FailKind> {
        match self {
            ReadFail::NotReady => None,
            ReadFail::Failed { kind, .. } => Some(*kind),
        }
    }
}

impl std::fmt::Display for ReadFail {
    /// Render the originating failure message or the not-ready sentinel text.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadFail::NotReady => f.write_str("replica not ready"),
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
/// Repeats of the same `(ctx, kind)` are collapsed — see [`WARN_REPEAT_GAP`].
pub(super) fn read_fail(ctx: &'static str, e: rusqlite::Error) -> ReadFail {
    let kind = classify(&e);
    log_throttled(ctx, kind, &e);
    ReadFail::Failed {
        kind,
        msg: Arc::from(format!("{e}")),
    }
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
pub(super) fn io_fail(ctx: &'static str, e: &std::io::Error) -> ReadFail {
    log_throttled(ctx, FailKind::Other, e);
    ReadFail::Failed {
        kind: FailKind::Other,
        msg: Arc::from(format!("{e}")),
    }
}

/// Log the first failure immediately and summarize repeats after the throttle window.
fn log_throttled(ctx: &'static str, kind: FailKind, e: &dyn std::fmt::Display) {
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
    if n > 0 {
        log::warn!("{ctx}: {e} (повторов подавлено: {n})");
    } else {
        log::warn!("{ctx}: {e}");
    }
}

#[cfg(test)]
/// Classification and warning-throttle regression tests.
mod tests;
