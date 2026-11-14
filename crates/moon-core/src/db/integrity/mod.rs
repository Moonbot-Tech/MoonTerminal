//! SQLite integrity checks of the reports replica.
//!
//! Individual reads only detect damage on pages they touch, so narrow period
//! windows can succeed while wider ones fail. Startup uses one cheap `quick_check`
//! gate, then one full `integrity_check` off the UI thread and publishes a pollable
//! verdict, including a check that could not run. Large replicas skip only the
//! synchronous gate; the background scan runs to completion whatever the file size,
//! because it is the only thing that can authorize the writer.
//!
//! The full check remains mandatory before the writer opens: `quick_check` skips
//! the index-vs-table content comparison, so a replica whose index row counts have
//! drifted out of step with the table would pass it. (Both catch outright page
//! damage — that part is not the differentiator.)

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags};

use super::FailKind;

/// Verdict of the replica integrity check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Integrity {
    /// The complete replica passed `PRAGMA integrity_check`.
    Ok,
    /// SQLite reported problems; each entry is one diagnostic line.
    Damaged(Vec<String>),
    /// No replica file exists; this is the only silent terminal state.
    NotPresent,
    /// The check itself could not run or complete. Never silent: an
    /// auto-detector that fails quietly is worse than none.
    CheckFailed(String),
}

/// Diagnostic entries are capped so a badly damaged file cannot produce an
/// unbounded report. This bounds output, not scan work.
const MAX_PROBLEM_ROWS: usize = 20;

/// Avoid scanning large replicas on the startup thread. The full background scan has neither a
/// size nor a time limit: skipping or cutting it would permanently disable recording for healthy
/// files.
const MAX_QUICK_CHECK_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Poll interval while the background full check is still running.
const POLL_HINT: Duration = Duration::from_millis(500);

/// Poll interval once the check has run long enough for its still-running notice.
const SLOW_POLL_HINT: Duration = Duration::from_secs(5);

static RESULT: OnceLock<Integrity> = OnceLock::new();
static LIVE_DAMAGE: OnceLock<Integrity> = OnceLock::new();
static LIVE_DAMAGE_ACTIVE: AtomicBool = AtomicBool::new(false);
pub(super) static WRITES_BLOCKED: AtomicBool = AtomicBool::new(false);
static WRITE_BARRIER: RwLock<()> = RwLock::new(());
#[cfg(test)]
static TEST_STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Current verdict, or `None` before startup or while the check is pending.
///
/// Borrows from the `OnceLock` rather than cloning: the Analytics window polls
/// this from `render`, and `Damaged` carries a `Vec<String>` that would then be
/// deep-cloned on every frame — including the frames a chart hover repaints.
///
/// Polled, never subscribed to, per the panel rules.
///
/// Returns:
///     Published live damage first, then the one-shot background verdict.
pub fn status() -> Option<&'static Integrity> {
    if LIVE_DAMAGE_ACTIVE.load(Ordering::Acquire) {
        LIVE_DAMAGE.get().or_else(|| RESULT.get())
    } else {
        RESULT.get()
    }
}

/// How long a poller should wait before asking [`status`] again.
///
/// The full check starts immediately, so a short interval notices its verdict without
/// delaying damage visibility behind the old fixed startup delay. A check that has run long
/// enough to be announced is a large replica with minutes still to go: the interval then
/// stretches, or a window polling it would repaint twice a second for all of that time.
pub fn poll_hint() -> Duration {
    if matches!(
        super::report_recovery::status(),
        Some(super::report_recovery::RecoveryNotice::CheckRunning)
    ) {
        SLOW_POLL_HINT
    } else {
        POLL_HINT
    }
}

/// Publish the first terminal verdict and block future writes after confirmed damage.
///
/// Args:
///     verdict: Completed integrity-check outcome.
pub(super) fn publish(verdict: Integrity) {
    if matches!(verdict, Integrity::Damaged(_)) {
        let _barrier = WRITE_BARRIER.write().unwrap_or_else(|p| p.into_inner());
        WRITES_BLOCKED.store(true, Ordering::Release);
        let _ = RESULT.set(verdict);
        return;
    }
    let _ = RESULT.set(verdict);
}

/// Return whether confirmed in-session corruption has disabled the report writer.
pub(super) fn writes_blocked() -> bool {
    WRITES_BLOCKED.load(Ordering::Acquire)
}

/// Clone corruption already confirmed by a preflight read.
///
/// Returns:
///     Active corruption verdict, or `None` after recovery or before any corrupt read.
pub(super) fn active_damage() -> Option<Integrity> {
    LIVE_DAMAGE_ACTIVE
        .load(Ordering::Acquire)
        .then(|| LIVE_DAMAGE.get().cloned())
        .flatten()
}

/// Hold the shared publication barrier while a committed batch is published and acknowledged.
///
/// Returns:
///     Read guard that prevents a damage verdict from becoming visible until ACK publication ends.
pub(super) fn writer_ack_guard() -> RwLockReadGuard<'static, ()> {
    WRITE_BARRIER.read().unwrap_or_else(|p| p.into_inner())
}

/// Decide whether one writer error requires a permanent fail-closed stop.
///
/// Args:
///     error: Error returned after the transaction-level retry allowance.
///
/// Returns:
///     `true` for a corruption-class error or a prior background damage verdict.
pub(super) fn writer_should_stop(error: &rusqlite::Error) -> bool {
    if super::read_fail::classify(error) == FailKind::Corrupt {
        record_corruption(error);
    }
    writes_blocked()
}

/// Publish corruption discovered outside the background checker and disable writes.
///
/// Args:
///     error: Corruption-class SQLite error from a reader or writer.
pub(super) fn record_corruption(error: &rusqlite::Error) {
    let _barrier = WRITE_BARRIER.write().unwrap_or_else(|p| p.into_inner());
    WRITES_BLOCKED.store(true, Ordering::Release);
    let _ = LIVE_DAMAGE.set(Integrity::Damaged(vec![error.to_string()]));
    LIVE_DAMAGE_ACTIVE.store(true, Ordering::Release);
}

/// Clear a preflight corruption latch after its exact file set was safely recovered.
///
/// This is intentionally unavailable to ordinary callers: only recovery publication plus atomic
/// source retirement can make the process safe to write again.
pub(super) fn clear_after_recovery() {
    let _barrier = WRITE_BARRIER.write().unwrap_or_else(|p| p.into_inner());
    LIVE_DAMAGE_ACTIVE.store(false, Ordering::Release);
    WRITES_BLOCKED.store(false, Ordering::Release);
}

/// Serialize tests that exercise the process-global corruption latch.
#[cfg(test)]
pub(super) fn test_state_guard() -> std::sync::MutexGuard<'static, ()> {
    TEST_STATE_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// Restore the process-global corruption latch to an inactive state for one serialized test.
#[cfg(test)]
pub(super) fn reset_test_state() {
    LIVE_DAMAGE_ACTIVE.store(false, Ordering::Release);
    WRITES_BLOCKED.store(false, Ordering::Release);
}

/// Run the full check to completion, including replicas too large for the synchronous cheap gate.
///
/// There is no deadline. The scan holds a WAL read snapshot for as long as it runs, which would
/// stall a running writer's `wal_checkpoint(TRUNCATE)` — but its only caller is the startup
/// recovery worker, and the writer does not open the replica until this verdict is in. The
/// two-minute limit that used to interrupt the scan therefore protected nothing, and a replica of
/// several gigabytes never finished inside it: every launch ended without a verdict and the whole
/// session recorded nothing (#1190).
///
/// Called on the recovery worker; tests may also drive it against an isolated file.
pub(crate) fn run(path: &Path) -> Integrity {
    // A metadata failure is not an absent replica; Path::exists would hide that distinction.
    match std::fs::metadata(path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Integrity::NotPresent,
        Err(e) => return Integrity::CheckFailed(format!("файл недоступен: {e}")),
    }

    let started = Instant::now();
    let mut conn = match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(c) => c,
        Err(e) => return classify_pragma_error(e, "открытие"),
    };
    let _ = conn.busy_timeout(Duration::from_secs(30));
    super::trace::install_on(&mut conn);

    // Take EVERY diagnostic row, and split each one: SQLite versions differ on
    // whether the N problems come back as N rows or one newline-joined row.
    let verdict = collect_problems(&conn);
    let elapsed = started.elapsed();

    match verdict {
        Ok(lines) if lines.len() == 1 && lines[0].trim() == "ok" => {
            log::info!("отчёты(integrity): целостность в порядке ({elapsed:?})");
            Integrity::Ok
        }
        Ok(lines) if lines.is_empty() => {
            Integrity::CheckFailed("проверка не вернула результата".into())
        }
        Ok(lines) => {
            for l in &lines {
                log::warn!("отчёты(integrity): {l}");
            }
            log::warn!("отчёты(integrity): реплика повреждена ({elapsed:?})");
            Integrity::Damaged(lines)
        }
        // Severe corruption can abort the pragma instead of producing diagnostic
        // rows, so corruption-class errors still map to `Damaged`.
        Err(e) => classify_pragma_error(e, "проверка"),
    }
}

/// Run the cheap startup gate without comparing every index entry to its table row.
///
/// Args:
///     path: Reports replica to inspect read-only before the first window opens.
///
/// Returns:
///     Page-corruption verdict used by synchronous recovery; an inconclusive result
///     leaves the later full check responsible for authorizing the writer.
pub(crate) fn quick(path: &Path) -> Integrity {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.len() > MAX_QUICK_CHECK_BYTES => {
            return Integrity::CheckFailed("replica is too large for startup validation".into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Integrity::NotPresent;
        }
        Err(error) => return Integrity::CheckFailed(format!("replica is unavailable: {error}")),
    }
    let mut conn = match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(conn) => conn,
        Err(error) => return classify_pragma_error(error, "quick open"),
    };
    let _ = conn.busy_timeout(Duration::from_secs(30));
    super::trace::install_on(&mut conn);
    match conn.query_row("PRAGMA main.quick_check(1)", [], |row| {
        row.get::<_, String>(0)
    }) {
        Ok(result) if result.trim() == "ok" => Integrity::Ok,
        Ok(result) => Integrity::Damaged(
            result
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect(),
        ),
        Err(error) => classify_pragma_error(error, "quick check"),
    }
}

/// All diagnostic lines of `PRAGMA integrity_check`, flattened.
fn collect_problems(conn: &Connection) -> Result<Vec<String>, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!("PRAGMA integrity_check({MAX_PROBLEM_ROWS})"))?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        for line in row?.lines() {
            let line = line.trim();
            if !line.is_empty() {
                out.push(line.to_string());
            }
        }
    }
    // The pragma argument bounds reported errors; cap the flattened lines too
    // in case a diagnostic value contains multiple lines.
    out.truncate(MAX_PROBLEM_ROWS.max(1));
    Ok(out)
}

/// Corruption reported by the pragma ITSELF rather than as diagnostic rows is
/// still `Damaged` — that is how the worst damage arrives.
fn classify_pragma_error(e: rusqlite::Error, stage: &str) -> Integrity {
    // Reuses the read path's corruption-code list: two copies would drift the
    // moment a code is added.
    if super::read_fail::classify(&e) == FailKind::Corrupt {
        log::warn!("отчёты(integrity): {stage} — SQLite сообщил о порче: {e}");
        return Integrity::Damaged(vec![format!("{e}")]);
    }
    log::warn!("отчёты(integrity): {stage} не выполнена: {e}");
    Integrity::CheckFailed(format!("{e}"))
}

#[cfg(test)]
mod tests;
