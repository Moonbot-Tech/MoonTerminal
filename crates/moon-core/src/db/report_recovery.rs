//! Crash-resilient replacement of a confirmed-damaged reports replica.
//!
//! Recovery is copy-first: the main SQLite file and its WAL/SHM sidecars are copied into a
//! staging directory, SHA-256 verified, described by versioned metadata, and published with one
//! directory rename. Only that published snapshot authorizes atomically retiring the live sources
//! into its `originals/` subdirectory. No damaged source is deleted. A completion marker lets the
//! next process distinguish a finished recovery from a crash during finalization.
//!
//! A dedicated SQLite transaction provides the process-lifetime interprocess lease. Every current
//! MoonTerminal writer must own the private [`ReportWritePermit`], so a second current process
//! cannot inspect, replace, or write the same portable replica concurrently.

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use anyhow::Context as _;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::integrity::{self, Integrity};
use crate::config::paths;
use crate::util::{now_unix_ms_i64, utc_stamp_compact};

/// Metadata schema written into every published recovery snapshot.
const METADATA_SCHEMA: u32 = 1;

/// Minimum interval between automatic replacements of independently damaged replicas.
const RECOVERY_COOLDOWN_MS: i64 = 24 * 60 * 60 * 1_000;

/// Prefix identifying published report-recovery snapshots.
const SNAPSHOT_PREFIX: &str = "reports-";

/// Prefix identifying unpublished staging directories.
const STAGING_PREFIX: &str = ".incoming-reports-";

/// Sequence making staging directories unique inside one process.
static STAGING_SEQ: AtomicU32 = AtomicU32::new(0);

/// Process-lifetime lease connection. The open exclusive transaction releases on process exit.
static LEASE: OnceLock<Mutex<Connection>> = OnceLock::new();

/// Write and maintenance access becomes live only after the full recovery preflight succeeds.
static ACCESS_ENABLED: AtomicBool = AtomicBool::new(false);

/// Read access opens after the cheap gate, before the full check completes.
static READ_ACCESS_ENABLED: AtomicBool = AtomicBool::new(false);

/// Process-lifetime recovery notice consumed by Report and Analytics.
static NOTICE: OnceLock<RecoveryNotice> = OnceLock::new();

/// Private capability required to start the reports writer.
///
/// Only [`prepare`] can construct this value. Its wait gate keeps every database write behind
/// the background full-integrity verdict without delaying the first window.
pub struct ReportWritePermit {
    /// Shared startup verdict signaled exactly once by recovery.
    gate: Arc<StartupGate>,
    /// Shared read-only connection dropped before recovery and held through writer initialization.
    anchor: Arc<Mutex<Option<Connection>>>,
}

/// Writer authorization published by the synchronous cheap gate or background full check.
struct StartupGate {
    /// `None` while the full check runs; `Some` is whether the writer may open the replica.
    allowed: Mutex<Option<bool>>,
    /// Wakes the writer when `allowed` becomes final.
    ready: Condvar,
}

/// Process-wide write verdict awaited by maintenance and Report preference persistence.
static WRITE_ACCESS: StartupGate = StartupGate {
    allowed: Mutex::new(None),
    ready: Condvar::new(),
};

impl StartupGate {
    /// Create a permit gate that the background full check must resolve.
    fn pending() -> Arc<Self> {
        Arc::new(Self {
            allowed: Mutex::new(None),
            ready: Condvar::new(),
        })
    }

    /// Create an already-resolved gate after synchronous recovery.
    fn resolved(allowed: bool) -> Arc<Self> {
        let gate = Self::pending();
        gate.finish(allowed);
        gate
    }

    /// Publish the final writer authorization and wake its thread.
    fn finish(&self, allowed: bool) {
        let mut slot = self.allowed.lock().unwrap_or_else(|p| p.into_inner());
        *slot = Some(allowed);
        self.ready.notify_all();
    }

    /// Wait without holding the UI thread, then return whether writes may begin.
    fn wait(&self) -> bool {
        let mut allowed = self.allowed.lock().unwrap_or_else(|p| p.into_inner());
        while allowed.is_none() {
            allowed = self.ready.wait(allowed).unwrap_or_else(|p| p.into_inner());
        }
        allowed.unwrap_or(false)
    }
}

impl ReportWritePermit {
    /// Wait for the full verdict and hand the writer its WAL anchor and authorization.
    pub(crate) fn wait(self) -> (bool, Arc<Mutex<Option<Connection>>>) {
        let allowed = self.gate.wait();
        (allowed, self.anchor)
    }
}

/// User-facing startup recovery state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryNotice {
    /// The process lease could not be acquired; recovery never started.
    LeaseUnavailable {
        /// Technical reason recorded for diagnostics.
        detail: String,
    },
    /// A damaged file set was preserved and replaced with a fresh replica.
    Recovered {
        /// Published directory containing the old main/WAL/SHM files and metadata.
        snapshot_dir: PathBuf,
    },
    /// Recovery deliberately left the current replica untouched.
    Blocked {
        /// Technical reason recorded for diagnostics.
        detail: String,
        /// Most relevant earlier snapshot, when one was found.
        snapshot_dir: Option<PathBuf>,
    },
    /// Recovery could not establish its safety guarantees.
    Failed {
        /// Technical reason recorded for diagnostics.
        detail: String,
    },
}

/// Internal preflight decision before it is mapped onto a writer permit and UI notice.
#[derive(Debug, Eq, PartialEq)]
enum Decision {
    /// The existing replica is healthy, absent, or could not be conclusively classified.
    Ready,
    /// A published snapshot was finalized and the original paths are ready for a fresh replica.
    Recovered(PathBuf),
    /// A safety policy intentionally refused automatic replacement.
    Blocked {
        /// Technical reason for refusing replacement.
        detail: String,
        /// Earlier snapshot associated with the refusal.
        snapshot_dir: Option<PathBuf>,
    },
    /// An I/O or metadata failure prevented safe recovery.
    Failed(String),
}

/// Versioned description of one published damaged-replica snapshot.
#[derive(Debug, Deserialize, Serialize)]
struct RecoveryMetadata {
    /// Metadata schema version.
    schema_version: u32,
    /// UTC Unix timestamp when recovery started.
    created_unix_ms: i64,
    /// Generic explanation that does not identify a storage vendor.
    cause: String,
    /// SQLite diagnostics returned by the integrity check.
    diagnostics: Vec<String>,
    /// Expected state and fingerprint of every main/WAL/SHM path.
    files: Vec<SnapshotFile>,
}

/// Fingerprint of one member of the reports-replica file set.
#[derive(Debug, Deserialize, Serialize)]
struct SnapshotFile {
    /// Base file name, never a path.
    name: String,
    /// Whether this member existed when the snapshot was assembled.
    present: bool,
    /// Exact source length, or zero for an absent member.
    size_bytes: u64,
    /// Lowercase SHA-256, or an empty string for an absent member.
    sha256: String,
}

/// Current recovery notice.
///
/// Returns:
///     Process-lifetime notice, or `None` when no recovery action was needed.
pub fn status() -> Option<&'static RecoveryNotice> {
    NOTICE.get()
}

/// Return whether this process owns the reports-replica lease and may write or checkpoint it.
///
/// Returns:
///     `true` only after the full integrity check authorized write and maintenance access.
pub(super) fn access_permitted() -> bool {
    LEASE.get().is_some() && ACCESS_ENABLED.load(Ordering::Acquire)
}

/// Require full write and maintenance authorization.
#[cfg(test)]
pub(crate) fn ensure_access() -> anyhow::Result<()> {
    anyhow::ensure!(
        access_permitted(),
        "reports replica access requires the process lease and completed integrity check"
    );
    Ok(())
}

/// Wait on a background writer thread until the full check authorizes replica writes.
pub fn wait_for_write_access() -> anyhow::Result<()> {
    anyhow::ensure!(
        WRITE_ACCESS.wait(),
        "reports replica writes require a conclusive full integrity check"
    );
    Ok(())
}

/// Require the cheap read gate while writes remain behind the full integrity check.
pub(crate) fn ensure_read_access() -> anyhow::Result<()> {
    anyhow::ensure!(
        LEASE.get().is_some() && READ_ACCESS_ENABLED.load(Ordering::Acquire),
        "reports replica reads require the process lease and cheap integrity gate"
    );
    Ok(())
}

/// Hold a read-only WAL connection until the asynchronous writer has initialized.
///
/// Early report preference reads still need read-write connections. Keeping this anchor alive
/// prevents their close from becoming SQLite's last-connection checkpoint on the UI thread.
fn startup_read_anchor(path: &Path) -> anyhow::Result<Option<Connection>> {
    match std::fs::metadata(path) {
        Ok(_) => {
            let conn =
                Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                    .with_context(|| format!("open read-only WAL anchor {}", path.display()))?;
            conn.query_row("PRAGMA main.schema_version", [], |row| row.get::<_, i64>(0))
                .context("initialize WAL anchor read")?;
            Ok(Some(conn))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("inspect WAL anchor path {}", path.display()))
        }
    }
}

/// Acquire the process lease, run the cheap recovery gate, and start one full check.
///
/// A lease failure publishes its own notice because recovery has not started in that case.
/// Confirmed page damage is still recovered synchronously. A cheap pass enables readers and the
/// first window immediately, while the returned permit keeps the writer closed until the
/// background full check and any recovery it requires have finished.
///
/// Returns:
///     A private writer permit after the cheap gate, or `None` on a fail-closed decision.
pub fn prepare() -> Option<ReportWritePermit> {
    let lease = match acquire_lease(&paths::reports_recovery_lease_path()) {
        Ok(lease) => lease,
        Err(error) => {
            let detail = format!("reports replica lease unavailable: {error:#}");
            log::error!("{detail}");
            let _ = NOTICE.set(RecoveryNotice::LeaseUnavailable { detail });
            WRITE_ACCESS.finish(false);
            return None;
        }
    };
    if LEASE.set(Mutex::new(lease)).is_err() {
        let detail = "reports recovery preflight was invoked more than once".to_string();
        log::error!("{detail}");
        let _ = NOTICE.set(RecoveryNotice::Failed { detail });
        WRITE_ACCESS.finish(false);
        return None;
    }

    let files = paths::reports_db_files();
    let recovery_root = paths::damaged_reports_dir();
    let decision = prepare_at(&files, &recovery_root, now_unix_ms_i64());
    match decision {
        Decision::Ready => {
            let anchor = match startup_read_anchor(&files[0]) {
                Ok(anchor) => anchor,
                Err(error) => {
                    let detail = format!("reports WAL startup anchor failed: {error:#}");
                    log::error!("{detail}");
                    let _ = NOTICE.set(RecoveryNotice::Failed { detail });
                    WRITE_ACCESS.finish(false);
                    return None;
                }
            };
            let anchor = Arc::new(Mutex::new(anchor));
            let thread_anchor = anchor.clone();
            ACCESS_ENABLED.store(false, Ordering::Release);
            READ_ACCESS_ENABLED.store(true, Ordering::Release);
            let gate = StartupGate::pending();
            let thread_gate = gate.clone();
            let spawned = std::thread::Builder::new()
                .name("reports-integrity".into())
                .spawn(move || {
                    let verdict = integrity::run(&files[0]);
                    let _readers = if matches!(verdict, Integrity::Damaged(_)) {
                        thread_anchor
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .take();
                        ACCESS_ENABLED.store(false, Ordering::Release);
                        READ_ACCESS_ENABLED.store(false, Ordering::Release);
                        match super::reader_budget::quiesce() {
                            Some(readers) => Some(readers),
                            None => {
                                integrity::publish(verdict.clone());
                                thread_gate.finish(false);
                                WRITE_ACCESS.finish(false);
                                let detail =
                                    "readers did not drain for report recovery".to_string();
                                log::error!("{detail}");
                                let _ = NOTICE.set(RecoveryNotice::Failed { detail });
                                return;
                            }
                        }
                    } else {
                        None
                    };
                    let decision = decide_integrity(
                        &files,
                        &recovery_root,
                        now_unix_ms_i64(),
                        verdict.clone(),
                    );
                    thread_gate.finish(finish_background(decision, verdict));
                });
            match spawned {
                Ok(_) => Some(ReportWritePermit { gate, anchor }),
                Err(error) => {
                    ACCESS_ENABLED.store(false, Ordering::Release);
                    READ_ACCESS_ENABLED.store(false, Ordering::Release);
                    gate.finish(false);
                    WRITE_ACCESS.finish(false);
                    let detail = format!("reports integrity thread did not start: {error}");
                    log::error!("{detail}");
                    let _ = NOTICE.set(RecoveryNotice::Failed { detail });
                    None
                }
            }
        }
        Decision::Recovered(_) => {
            let allowed = finish_background(decision, Integrity::NotPresent);
            Some(ReportWritePermit {
                gate: StartupGate::resolved(allowed),
                anchor: Arc::new(Mutex::new(None)),
            })
        }
        Decision::Blocked {
            detail,
            snapshot_dir,
        } => {
            WRITE_ACCESS.finish(false);
            log::error!("reports: automatic recovery blocked: {detail}");
            let _ = NOTICE.set(RecoveryNotice::Blocked {
                detail,
                snapshot_dir,
            });
            None
        }
        Decision::Failed(detail) => {
            WRITE_ACCESS.finish(false);
            log::error!("reports: automatic recovery failed closed: {detail}");
            let _ = NOTICE.set(RecoveryNotice::Failed { detail });
            None
        }
    }
}

/// Return whether the full check completed conclusively rather than skipping or timing out.
fn conclusive(verdict: &Integrity) -> bool {
    matches!(verdict, Integrity::Ok | Integrity::NotPresent)
}

/// Apply the background full-check decision and publish its final writer authorization.
///
/// Args:
///     decision: Recovery result after the one full scan.
///     verdict: Full-scan diagnostics retained for status and write blocking.
///
/// Returns:
///     `true` only after a conclusive pass or completed recovery with no damage latched.
fn finish_background(decision: Decision, verdict: Integrity) -> bool {
    let allowed = match decision {
        Decision::Ready => {
            let allowed = conclusive(&verdict) && !integrity::writes_blocked();
            integrity::publish(verdict);
            ACCESS_ENABLED.store(allowed, Ordering::Release);
            READ_ACCESS_ENABLED.store(allowed, Ordering::Release);
            allowed
        }
        Decision::Recovered(snapshot_dir) => {
            integrity::clear_after_recovery();
            integrity::publish(Integrity::NotPresent);
            ACCESS_ENABLED.store(true, Ordering::Release);
            READ_ACCESS_ENABLED.store(true, Ordering::Release);
            log::warn!(
                "reports: damaged replica preserved in {}; starting a fresh replica",
                snapshot_dir.display()
            );
            let _ = NOTICE.set(RecoveryNotice::Recovered { snapshot_dir });
            true
        }
        Decision::Blocked {
            detail,
            snapshot_dir,
        } => {
            ACCESS_ENABLED.store(false, Ordering::Release);
            READ_ACCESS_ENABLED.store(false, Ordering::Release);
            integrity::publish(verdict);
            log::error!("reports: automatic recovery blocked: {detail}");
            let _ = NOTICE.set(RecoveryNotice::Blocked {
                detail,
                snapshot_dir,
            });
            false
        }
        Decision::Failed(detail) => {
            ACCESS_ENABLED.store(false, Ordering::Release);
            READ_ACCESS_ENABLED.store(false, Ordering::Release);
            integrity::publish(verdict);
            log::error!("reports: automatic recovery failed closed: {detail}");
            let _ = NOTICE.set(RecoveryNotice::Failed { detail });
            false
        }
    };
    WRITE_ACCESS.finish(allowed);
    allowed
}

/// Open the coordination database and begin an exclusive process-lifetime transaction.
///
/// Args:
///     path: Dedicated SQLite path used only for recovery/writer coordination.
///
/// Returns:
///     The connection holding the exclusive lease.
fn acquire_lease(path: &Path) -> anyhow::Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create lease directory {}", parent.display()))?;
    }
    let conn =
        Connection::open(path).with_context(|| format!("open reports lease {}", path.display()))?;
    conn.busy_timeout(Duration::from_millis(250))
        .context("configure reports lease timeout")?;
    conn.execute_batch("BEGIN EXCLUSIVE TRANSACTION")
        .context("another MoonTerminal process owns the reports replica")?;
    Ok(conn)
}

/// Testable recovery core with injected paths and clock.
///
/// Args:
///     files: Main, WAL, and SHM paths in that exact order.
///     recovery_root: Directory for published damaged-replica snapshots.
///     now_ms: Current UTC Unix timestamp in milliseconds.
///
/// Returns:
///     A fail-closed startup decision.
fn prepare_at(files: &[PathBuf; 3], recovery_root: &Path, now_ms: i64) -> Decision {
    if let Err(error) = cleanup_abandoned_staging(files, recovery_root) {
        return Decision::Failed(format!("{error:#}"));
    }
    match resume_pending(files, recovery_root, now_ms) {
        Ok(Some(snapshot)) => return Decision::Recovered(snapshot),
        Ok(None) => {}
        Err(error) => return Decision::Failed(format!("{error:#}")),
    }

    let verdict = integrity::active_damage().unwrap_or_else(|| integrity::quick(&files[0]));
    decide_integrity(files, recovery_root, now_ms, verdict)
}

/// Remove only recognized files from unpublished staging left by a crashed recovery.
///
/// The process lease makes every matching staging directory abandoned. Cleanup is deliberately
/// non-recursive and refuses a symlinked root; an unexpected child preserves the directory for
/// manual inspection instead of allowing file synchronization software to broaden deletion.
///
/// Args:
///     files: Canonical report paths whose base names are recognized staging members.
///     recovery_root: Parent directory containing staging and published snapshots.
///
/// Returns:
///     Success after every safely recognizable abandoned staging directory is cleaned or retained.
fn cleanup_abandoned_staging(files: &[PathBuf; 3], recovery_root: &Path) -> anyhow::Result<()> {
    let entries = match std::fs::symlink_metadata(recovery_root) {
        Ok(metadata) if metadata.is_dir() => std::fs::read_dir(recovery_root)?,
        Ok(_) => anyhow::bail!(
            "recovery root is not a regular directory: {}",
            recovery_root.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir()
            || !entry
                .file_name()
                .to_string_lossy()
                .starts_with(STAGING_PREFIX)
        {
            continue;
        }
        let staging = entry.path();
        for source in files {
            let Some(name) = source.file_name() else {
                anyhow::bail!("report source has no file name: {}", source.display());
            };
            remove_staging_file(&staging.join(name))?;
        }
        remove_staging_file(&paths::report_recovery_metadata_path(&staging))?;
        let originals = paths::report_recovery_originals_dir(&staging);
        match std::fs::symlink_metadata(&originals) {
            Ok(metadata) if metadata.is_dir() => {
                let _ = std::fs::remove_dir(&originals);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        match std::fs::remove_dir(&staging) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                log::warn!(
                    "reports: abandoned recovery staging contains unexpected files and was \
                     preserved: {}",
                    staging.display()
                );
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// Remove one recognized staging member without following links or removing directories.
///
/// Args:
///     path: Exact expected file path inside unpublished staging.
///
/// Returns:
///     Success when the path is absent, removed as a file/link, or retained as a directory.
fn remove_staging_file(path: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_dir() => std::fs::remove_file(path)?,
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

/// Apply one already-obtained integrity verdict to the recovery policy.
///
/// Args:
///     files: Current main/WAL/SHM paths.
///     recovery_root: Directory for published snapshots.
///     now_ms: Current UTC Unix timestamp in milliseconds.
///     verdict: Bounded integrity-check outcome.
///
/// Returns:
///     Recovery decision after cooldown and snapshot handling.
fn decide_integrity(
    files: &[PathBuf; 3],
    recovery_root: &Path,
    now_ms: i64,
    verdict: Integrity,
) -> Decision {
    match verdict {
        Integrity::Ok | Integrity::NotPresent => Decision::Ready,
        Integrity::TimeLimitExceeded => {
            log::warn!(
                "reports: startup integrity preflight hit its time limit; leaving the replica untouched"
            );
            Decision::Ready
        }
        Integrity::CheckFailed(reason) => {
            log::warn!(
                "reports: startup integrity preflight was inconclusive; leaving the replica \
                 untouched: {reason}"
            );
            Decision::Ready
        }
        Integrity::Damaged(diagnostics) => match recent_snapshot(recovery_root, now_ms) {
            Ok(Some(snapshot)) => Decision::Blocked {
                detail: format!(
                    "the replica was damaged again within {} hours; current files were left \
                     untouched",
                    RECOVERY_COOLDOWN_MS / 3_600_000
                ),
                snapshot_dir: Some(snapshot),
            },
            Ok(None) => match create_snapshot(files, recovery_root, now_ms, diagnostics) {
                Ok(snapshot) => Decision::Recovered(snapshot),
                Err(error) => Decision::Failed(format!("{error:#}")),
            },
            Err(error) => Decision::Failed(format!("{error:#}")),
        },
    }
}

/// Resume the sole published snapshot whose source-removal marker is missing.
///
/// Args:
///     files: Current main/WAL/SHM paths.
///     recovery_root: Directory containing published snapshots.
///     now_ms: Current UTC Unix timestamp used when resumed finalization completes.
///
/// Returns:
///     The finalized snapshot path, or `None` when no interrupted finalization exists.
fn resume_pending(
    files: &[PathBuf; 3],
    recovery_root: &Path,
    now_ms: i64,
) -> anyhow::Result<Option<PathBuf>> {
    let pending = pending_snapshots(recovery_root)?;
    if pending.len() > 1 {
        anyhow::bail!(
            "multiple unfinished report recoveries require manual inspection: {}",
            recovery_root.display()
        );
    }
    let Some(snapshot) = pending.into_iter().next() else {
        return Ok(None);
    };
    finalize_snapshot(files, &snapshot, now_ms)
        .with_context(|| format!("resume recovery {}", snapshot.display()))?;
    Ok(Some(snapshot))
}

/// List published snapshots that lack the finalization marker.
///
/// Args:
///     recovery_root: Directory containing published snapshots.
///
/// Returns:
///     Recognized unfinished snapshot directories.
fn pending_snapshots(recovery_root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(recovery_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(anyhow::Error::from(error).context(format!(
                "read recovery directory {}",
                recovery_root.display()
            )));
        }
    };
    let mut pending = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(SNAPSHOT_PREFIX) {
            continue;
        }
        let path = entry.path();
        let marker = paths::report_recovery_finalized_path(&path);
        match std::fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.is_file() && read_finalized_ms(&path).is_ok() => {}
            Ok(metadata) if metadata.is_file() => pending.push(path),
            Ok(_) => anyhow::bail!(
                "recovery finalization marker is not a file: {}",
                path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => pending.push(path),
            Err(error) => return Err(error.into()),
        }
    }
    pending.sort();
    Ok(pending)
}

/// Find a completed recovery inside the automatic-recovery cooldown.
///
/// Args:
///     recovery_root: Directory containing published snapshots.
///     now_ms: Current UTC Unix timestamp in milliseconds.
///
/// Returns:
///     The newest recent snapshot, if one exists.
fn recent_snapshot(recovery_root: &Path, now_ms: i64) -> anyhow::Result<Option<PathBuf>> {
    let entries = match std::fs::read_dir(recovery_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut newest: Option<(i64, PathBuf)> = None;
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir()
            || !entry
                .file_name()
                .to_string_lossy()
                .starts_with(SNAPSHOT_PREFIX)
        {
            continue;
        }
        let snapshot = entry.path();
        let marker = paths::report_recovery_finalized_path(&snapshot);
        match std::fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => anyhow::bail!(
                "recovery finalization marker is not a regular file: {}",
                marker.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        }
        let _ = read_metadata(&snapshot)?;
        let finalized_ms = read_finalized_ms(&snapshot)?;
        let age = now_ms.saturating_sub(finalized_ms);
        if age <= RECOVERY_COOLDOWN_MS
            && newest
                .as_ref()
                .is_none_or(|(timestamp, _)| finalized_ms > *timestamp)
        {
            newest = Some((finalized_ms, snapshot));
        }
    }
    Ok(newest.map(|(_, path)| path))
}

/// Copy, verify, publish, and finalize one confirmed-damaged file set.
///
/// Args:
///     files: Current main/WAL/SHM paths.
///     recovery_root: Directory for published snapshots.
///     now_ms: Recovery timestamp.
///     diagnostics: SQLite integrity diagnostics.
///
/// Returns:
///     Published and finalized snapshot directory.
fn create_snapshot(
    files: &[PathBuf; 3],
    recovery_root: &Path,
    now_ms: i64,
    diagnostics: Vec<String>,
) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(recovery_root)
        .with_context(|| format!("create recovery directory {}", recovery_root.display()))?;
    let root_metadata = std::fs::symlink_metadata(recovery_root)?;
    if !root_metadata.is_dir() {
        anyhow::bail!(
            "recovery root is not a regular directory: {}",
            recovery_root.display()
        );
    }
    let staging = recovery_root.join(format!(
        "{STAGING_PREFIX}{}-{}",
        std::process::id(),
        STAGING_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&staging)
        .with_context(|| format!("create recovery staging {}", staging.display()))?;
    std::fs::create_dir(paths::report_recovery_originals_dir(&staging)).with_context(|| {
        format!(
            "create recovery originals directory {}",
            paths::report_recovery_originals_dir(&staging).display()
        )
    })?;

    let assembled = (|| -> anyhow::Result<RecoveryMetadata> {
        let mut snapshots = Vec::with_capacity(files.len());
        for source in files {
            snapshots.push(copy_and_verify(source, &staging)?);
        }
        let metadata = RecoveryMetadata {
            schema_version: METADATA_SCHEMA,
            created_unix_ms: now_ms,
            cause: "SQLite reported database corruption. Storage errors, interrupted writes, or \
                    file synchronization software can be involved."
                .to_string(),
            diagnostics,
            files: snapshots,
        };
        write_metadata(&staging, &metadata)?;
        Ok(metadata)
    })();
    if let Err(error) = assembled {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }

    let snapshot = match publish_snapshot(&staging, recovery_root, now_ms) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    finalize_snapshot(files, &snapshot, now_ms)
        .with_context(|| format!("finalize published snapshot {}", snapshot.display()))?;
    Ok(snapshot)
}

/// Copy one source member and prove that the copy has identical length and SHA-256.
///
/// Args:
///     source: Main, WAL, or SHM source path.
///     staging: Unpublished snapshot directory.
///
/// Returns:
///     Serializable source/copy fingerprint, including an explicit absent state.
fn copy_and_verify(source: &Path, staging: &Path) -> anyhow::Result<SnapshotFile> {
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            anyhow::anyhow!("report source has no UTF-8 file name: {}", source.display())
        })?
        .to_string();
    let source_metadata = match std::fs::symlink_metadata(source) {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => anyhow::bail!("report source is not a file: {}", source.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SnapshotFile {
                name,
                present: false,
                size_bytes: 0,
                sha256: String::new(),
            });
        }
        Err(error) => {
            return Err(anyhow::Error::from(error)
                .context(format!("read report source {}", source.display())));
        }
    };
    let destination = staging.join(&name);
    let copied = std::fs::copy(source, &destination).with_context(|| {
        format!(
            "copy report source {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&destination)?
        .sync_all()?;
    let source_hash = sha256_file(source)?;
    let destination_hash = sha256_file(&destination)?;
    if copied != source_metadata.len()
        || source_metadata.len() != destination_hash.0
        || source_hash != destination_hash
    {
        anyhow::bail!(
            "report source changed while it was copied: {}",
            source.display()
        );
    }
    Ok(SnapshotFile {
        name,
        present: true,
        size_bytes: source_hash.0,
        sha256: source_hash.1,
    })
}

/// Write and durably flush versioned metadata inside an unpublished snapshot.
///
/// Args:
///     snapshot: Staging snapshot directory.
///     metadata: Complete file fingerprints and diagnostics.
///
/// Returns:
///     Success after metadata bytes and their file handle are flushed.
fn write_metadata(snapshot: &Path, metadata: &RecoveryMetadata) -> anyhow::Result<()> {
    let path = paths::report_recovery_metadata_path(snapshot);
    let bytes = serde_json::to_vec_pretty(metadata)?;
    let mut file = std::fs::File::create(&path)
        .with_context(|| format!("create recovery metadata {}", path.display()))?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

/// Publish a complete staging directory with one rename.
///
/// Args:
///     staging: Fully assembled unpublished directory.
///     recovery_root: Parent directory for final snapshots.
///     now_ms: Recovery timestamp used in the readable directory name.
///
/// Returns:
///     Unique published directory path.
fn publish_snapshot(staging: &Path, recovery_root: &Path, now_ms: i64) -> anyhow::Result<PathBuf> {
    let base = format!("{SNAPSHOT_PREFIX}{}", utc_stamp_compact(now_ms));
    let mut last_error = None;
    for attempt in 0..=99u32 {
        let name = if attempt == 0 {
            base.clone()
        } else {
            format!("{base}-{attempt:02}")
        };
        let destination = recovery_root.join(name);
        if destination.exists() {
            continue;
        }
        match std::fs::rename(staging, &destination) {
            Ok(()) => return Ok(destination),
            Err(error) => last_error = Some(error),
        }
    }
    match last_error {
        Some(error) => Err(error.into()),
        None => anyhow::bail!("all recovery snapshot names are occupied for {base}"),
    }
}

/// Atomically retire current sources into a published snapshot and mark it complete.
///
/// Args:
///     files: Current main/WAL/SHM paths.
///     snapshot: Published snapshot authorizing source retirement.
///     finalized_ms: UTC Unix timestamp recorded when retirement completes.
///
/// Returns:
///     Success after every expected source is retired and both copies verify.
fn finalize_snapshot(
    files: &[PathBuf; 3],
    snapshot: &Path,
    finalized_ms: i64,
) -> anyhow::Result<()> {
    let metadata = read_metadata(snapshot)?;
    validate_metadata_files(files, &metadata)?;
    validate_snapshot_files(snapshot, &metadata)?;
    let originals = paths::report_recovery_originals_dir(snapshot);
    let originals_metadata = std::fs::symlink_metadata(&originals)
        .with_context(|| format!("inspect recovery originals {}", originals.display()))?;
    if !originals_metadata.is_dir() {
        anyhow::bail!(
            "recovery originals path is not a directory: {}",
            originals.display()
        );
    }

    for (source, expected) in files.iter().zip(&metadata.files) {
        let retired = originals.join(&expected.name);
        let live_state = regular_file_state(source)?;
        let retired_state = regular_file_state(&retired)?;
        if !expected.present {
            if live_state || retired_state {
                anyhow::bail!(
                    "unexpected report file appeared during recovery: {}",
                    source.display()
                );
            }
            continue;
        }
        if live_state && retired_state {
            anyhow::bail!(
                "report source exists at both live and retired paths: {}",
                source.display()
            );
        }
        if live_state {
            std::fs::rename(source, &retired).with_context(|| {
                format!(
                    "retire report source {} into {}",
                    source.display(),
                    retired.display()
                )
            })?;
        } else if !retired_state {
            anyhow::bail!(
                "report source is missing from both live and retired paths: {}",
                source.display()
            );
        }
        let current = sha256_file(&retired)?;
        if current.0 != expected.size_bytes || current.1 != expected.sha256 {
            anyhow::bail!(
                "retired report source no longer matches published snapshot: {}",
                retired.display()
            );
        }
    }
    validate_snapshot_files(snapshot, &metadata)?;
    for source in files {
        if regular_file_state(source)? {
            anyhow::bail!(
                "report source reappeared at its live path during recovery: {}",
                source.display()
            );
        }
    }
    write_finalized_marker(snapshot, finalized_ms)?;
    Ok(())
}

/// Return whether a path is a regular non-symlink file, rejecting every other existing type.
///
/// Args:
///     path: Candidate live, retired, or snapshot path.
///
/// Returns:
///     `true` for a regular file, `false` only when the path is absent.
fn regular_file_state(path: &Path) -> anyhow::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => anyhow::bail!("recovery path is not a regular file: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Prove that published files still match the fingerprints authorizing source retirement.
///
/// Args:
///     snapshot: Published snapshot directory.
///     metadata: Parsed and schema-validated snapshot metadata.
///
/// Returns:
///     Success when each present copy matches and each absent member remains absent.
fn validate_snapshot_files(snapshot: &Path, metadata: &RecoveryMetadata) -> anyhow::Result<()> {
    for expected in &metadata.files {
        let copy = snapshot.join(&expected.name);
        match std::fs::symlink_metadata(&copy) {
            Ok(_current) if !expected.present => {
                anyhow::bail!("unexpected file in recovery snapshot: {}", copy.display())
            }
            Ok(current) if !current.is_file() => {
                anyhow::bail!("recovery snapshot member is not a file: {}", copy.display())
            }
            Ok(_) => {
                let actual = sha256_file(&copy)?;
                if actual.0 != expected.size_bytes || actual.1 != expected.sha256 {
                    anyhow::bail!(
                        "recovery snapshot member failed verification: {}",
                        copy.display()
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !expected.present => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                anyhow::bail!("recovery snapshot member is missing: {}", copy.display())
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// Read and validate one published snapshot's metadata schema.
///
/// Args:
///     snapshot: Published snapshot directory.
///
/// Returns:
///     Parsed metadata with the supported schema version.
fn read_metadata(snapshot: &Path) -> anyhow::Result<RecoveryMetadata> {
    let path = paths::report_recovery_metadata_path(snapshot);
    let bytes = std::fs::read(&path)
        .with_context(|| format!("read recovery metadata {}", path.display()))?;
    let metadata: RecoveryMetadata = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse recovery metadata {}", path.display()))?;
    if metadata.schema_version != METADATA_SCHEMA {
        anyhow::bail!(
            "unsupported recovery metadata schema {} in {}",
            metadata.schema_version,
            path.display()
        );
    }
    Ok(metadata)
}

/// Validate metadata file order and names against the canonical report set.
///
/// Args:
///     files: Canonical main/WAL/SHM paths.
///     metadata: Parsed snapshot metadata.
///
/// Returns:
///     Success when metadata contains the canonical members in canonical order.
fn validate_metadata_files(
    files: &[PathBuf; 3],
    metadata: &RecoveryMetadata,
) -> anyhow::Result<()> {
    if metadata.files.len() != files.len() {
        anyhow::bail!(
            "recovery metadata contains {} files instead of {}",
            metadata.files.len(),
            files.len()
        );
    }
    for (source, expected) in files.iter().zip(&metadata.files) {
        let actual_name = source.file_name().and_then(|name| name.to_str());
        if actual_name != Some(expected.name.as_str()) {
            anyhow::bail!("recovery metadata file order does not match canonical report paths");
        }
    }
    Ok(())
}

/// Create and flush the timestamp marker proving original-file retirement completed.
///
/// Args:
///     snapshot: Published snapshot directory.
///     finalized_ms: UTC Unix timestamp of successful finalization.
///
/// Returns:
///     Success after the marker exists and its contents are flushed.
fn write_finalized_marker(snapshot: &Path, finalized_ms: i64) -> anyhow::Result<()> {
    anyhow::ensure!(
        finalized_ms >= 0,
        "recovery finalization timestamp cannot be negative"
    );
    let path = paths::report_recovery_finalized_path(snapshot);
    let staging = paths::report_recovery_finalizing_path(snapshot);
    remove_staging_file(&staging)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)
        .with_context(|| format!("create recovery finalization marker {}", staging.display()))?;
    writeln!(file, "{finalized_ms}")?;
    file.sync_all()?;
    drop(file);

    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && read_finalized_ms(snapshot).is_ok() => {
            std::fs::remove_file(&staging)?;
            return Ok(());
        }
        Ok(metadata) if metadata.is_file() => std::fs::remove_file(&path)?,
        Ok(_) => anyhow::bail!(
            "recovery finalization marker is not a regular file: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    std::fs::rename(&staging, &path).with_context(|| {
        format!(
            "publish recovery finalization marker {} to {}",
            staging.display(),
            path.display()
        )
    })?;
    Ok(())
}

/// Read and validate the completion time from a finalized recovery marker.
///
/// Args:
///     snapshot: Published snapshot directory.
///
/// Returns:
///     Non-negative UTC Unix timestamp in milliseconds.
fn read_finalized_ms(snapshot: &Path) -> anyhow::Result<i64> {
    let path = paths::report_recovery_finalized_path(snapshot);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("read recovery finalization marker {}", path.display()))?;
    let finalized_ms: i64 = text
        .trim()
        .parse()
        .with_context(|| format!("parse recovery finalization marker {}", path.display()))?;
    anyhow::ensure!(
        finalized_ms >= 0,
        "recovery finalization timestamp cannot be negative"
    );
    Ok(finalized_ms)
}

/// Stream a file into SHA-256 without loading the replica into memory.
///
/// Args:
///     path: File to fingerprint.
///
/// Returns:
///     Exact byte length and lowercase SHA-256.
fn sha256_file(path: &Path) -> anyhow::Result<(u64, String)> {
    let mut file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    // Keep the large I/O buffer off the fixed-size Windows startup thread stack.
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut hex, "{byte:02x}")?;
    }
    Ok((total, hex))
}

#[cfg(test)]
mod tests;
