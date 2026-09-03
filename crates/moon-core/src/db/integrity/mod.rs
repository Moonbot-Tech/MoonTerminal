//! Background SQLite integrity check of the reports replica.
//!
//! Individual reads only detect damage on pages they touch, so narrow period
//! windows can succeed while wider ones fail. This module attempts one bounded
//! full-replica check per process off the UI thread and publishes a pollable
//! verdict, including setup, size-limit, and timeout failures.
//!
//! `integrity_check` and NOT `quick_check`: the faster pragma skips the
//! index-vs-table content comparison, so a replica whose index row counts have
//! drifted out of step with the table would pass it. (Both catch outright page
//! damage — that part is not the differentiator.)

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags};

use super::FailKind;
use crate::config::paths;

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

/// Provisional ceiling above which the full scan is skipped rather than
/// competing with the writer for disk for minutes at every launch. Revisit with
/// the elapsed-time telemetry this module logs on every run — the number is a
/// bound, not a measurement.
const MAX_CHECK_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Delay before scanning, so the check does not contend with the initial sync
/// burst right after startup.
const START_DELAY: Duration = Duration::from_secs(20);

/// Hard ceiling on the scan itself. The reader must let go of its WAL snapshot
/// even on a pathological file, or the writer's checkpoint never completes.
const MAX_SCAN_TIME: Duration = Duration::from_secs(120);

static RESULT: OnceLock<Integrity> = OnceLock::new();
static LIVE_DAMAGE: OnceLock<Integrity> = OnceLock::new();
static LIVE_DAMAGE_ACTIVE: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);
pub(super) static WRITES_BLOCKED: AtomicBool = AtomicBool::new(false);
static WRITE_BARRIER: RwLock<()> = RwLock::new(());
#[cfg(test)]
static TEST_STATE_LOCK: Mutex<()> = Mutex::new(());

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
/// A normal worker cannot finish before the startup delay, so polling more often
/// only forces pointless repaints. Callers must still accept an immediate
/// `CheckFailed` verdict when the worker thread cannot start.
pub fn poll_hint() -> Duration {
    START_DELAY
}

/// Start the one-shot check. Idempotent: later calls are no-ops.
///
/// The function returns immediately after spawning, or after publishing a setup failure.
pub fn spawn_check() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    if !super::report_recovery::access_permitted() {
        publish(Integrity::CheckFailed(
            "reports replica access is disabled because this process does not own the lease"
                .to_string(),
        ));
        return;
    }
    let path = paths::reports_db_path();
    let spawned = std::thread::Builder::new()
        .name("reports-integrity".into())
        .spawn(move || {
            std::thread::sleep(START_DELAY);
            publish(run(&path));
        });
    if let Err(e) = spawned {
        // A thread we could not start must not leave `status()` at `Running`
        // forever — that would read as "still checking" for the whole session.
        publish(Integrity::CheckFailed(format!(
            "поток проверки не запустился: {e}"
        )));
    }
}

/// Publish the first terminal verdict and block future writes after confirmed damage.
///
/// Args:
///     verdict: Completed integrity-check outcome.
fn publish(verdict: Integrity) {
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

/// Run the check synchronously. Public to the crate so tests can drive it
/// against a fabricated file without going through the thread.
pub(crate) fn run(path: &Path) -> Integrity {
    // ONE metadata call answers all three questions — present, how big, or why
    // not readable. `Path::exists` is unsuitable for the first question for
    // the same reason `db::open_reader` rejects it: it reports false for a
    // permission or metadata error too, which would make an unreadable replica
    // read as an absent one HERE while the read path reports it as a failure.
    // Two answers to "does the replica exist?" in one subsystem is worse than
    // either answer.
    match std::fs::metadata(path) {
        Ok(m) if m.len() > MAX_CHECK_BYTES => {
            log::info!(
                "отчёты(integrity): проверка пропущена — файл {} МБ больше порога {} МБ",
                m.len() / (1024 * 1024),
                MAX_CHECK_BYTES / (1024 * 1024)
            );
            return Integrity::CheckFailed("файл слишком велик для полной проверки".into());
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Integrity::NotPresent,
        Err(e) => return Integrity::CheckFailed(format!("файл недоступен: {e}")),
    }

    let started = Instant::now();
    let conn = match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(c) => c,
        Err(e) => return classify_pragma_error(e, "открытие"),
    };
    let _ = conn.busy_timeout(Duration::from_secs(30));
    super::trace::install_on(&conn);

    // HARD DEADLINE. `busy_timeout` bounds lock waiting, not statement runtime,
    // and the scan holds a WAL read snapshot for its whole duration — which
    // blocks the writer's `wal_checkpoint(TRUNCATE)` from reclaiming space, so
    // an unbounded scan lets the -wal file grow while the writer stalls. The
    // watchdog interrupts the statement so the reader always lets go.
    let handle = conn.get_interrupt_handle();
    // Condvar rather than a sleep-poll loop: the watchdog wakes exactly once,
    // and a healthy sub-second scan is not followed by a poll-interval tail
    // before `join` returns.
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let watchdog = {
        let done = Arc::clone(&done);
        std::thread::Builder::new()
            .name("reports-integrity-watchdog".into())
            .spawn(move || {
                let (lock, cv) = &*done;
                let finished = lock.lock().unwrap_or_else(|p| p.into_inner());
                let (finished, timeout) = cv
                    .wait_timeout_while(finished, MAX_SCAN_TIME, |done| !*done)
                    .unwrap_or_else(|p| p.into_inner());
                if timeout.timed_out() && !*finished {
                    log::warn!(
                        "отчёты(integrity): превышен лимит {MAX_SCAN_TIME:?} — проверка прервана"
                    );
                    handle.interrupt();
                }
            })
    };

    // The deadline is only as real as the thread enforcing it, so the scan may
    // not start until that thread exists. Running it anyway would hold a WAL
    // read snapshot for an unbounded time and stall the writer's checkpoint —
    // precisely the harm the deadline is here to prevent.
    let watchdog = match watchdog {
        Ok(w) => w,
        Err(e) => {
            log::warn!("отчёты(integrity): сторож не запустился ({e}) — проверка не выполнена");
            return Integrity::CheckFailed(format!("сторож не запустился: {e}"));
        }
    };

    // Take EVERY diagnostic row, and split each one: SQLite versions differ on
    // whether the N problems come back as N rows or one newline-joined row.
    let verdict = collect_problems(&conn);
    {
        let (lock, cv) = &*done;
        *lock.lock().unwrap_or_else(|p| p.into_inner()) = true;
        cv.notify_all();
    }
    let _ = watchdog.join();
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
