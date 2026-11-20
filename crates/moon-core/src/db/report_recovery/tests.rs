//! Recovery state-machine tests over isolated temporary file sets.

use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

/// Sequence making each test directory unique within the test process.
static TEST_SEQ: AtomicU32 = AtomicU32::new(0);

/// Build an isolated temporary root for one recovery scenario.
///
/// Args:
///     tag: Human-readable scenario label.
///
/// Returns:
///     Newly created temporary directory.
fn test_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "moon-report-recovery-{tag}-{}-{}",
        std::process::id(),
        TEST_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// Return the canonical main/WAL/SHM paths under an injected database directory.
///
/// Args:
///     root: Temporary database directory.
///
/// Returns:
///     Main, WAL, and SHM paths in production order.
fn test_files(root: &Path) -> [PathBuf; 3] {
    [
        root.join("reports.sqlite"),
        root.join("reports.sqlite-wal"),
        root.join("reports.sqlite-shm"),
    ]
}

/// Write deterministic bytes to one test file.
///
/// Args:
///     path: Destination file.
///     bytes: Independent expected payload.
fn write_bytes(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
}

/// Assemble and publish a snapshot without finalization to model a process crash.
///
/// Args:
///     files: Current main/WAL/SHM paths.
///     recovery_root: Snapshot parent.
///     now_ms: Deterministic recovery timestamp.
///
/// Returns:
///     Published directory lacking the finalization marker.
fn publish_pending(files: &[PathBuf; 3], recovery_root: &Path, now_ms: i64) -> PathBuf {
    std::fs::create_dir_all(recovery_root).unwrap();
    let staging = recovery_root.join(format!(
        "{STAGING_PREFIX}test-{}",
        TEST_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&staging).unwrap();
    std::fs::create_dir(paths::report_recovery_originals_dir(&staging)).unwrap();
    let metadata = RecoveryMetadata {
        schema_version: METADATA_SCHEMA,
        created_unix_ms: now_ms,
        cause: "test fixture".to_string(),
        diagnostics: vec!["fixture corruption".to_string()],
        files: files
            .iter()
            .map(|source| copy_and_verify(source, &staging).unwrap())
            .collect(),
    };
    write_metadata(&staging, &metadata).unwrap();
    publish_snapshot(&staging, recovery_root, now_ms).unwrap()
}

/// `db/report_recovery.rs:create_snapshot` must include WAL and SHM in the published snapshot.
///
/// Removing either sidecar from the canonical copy loop makes the corresponding independent byte
/// assertion fail; replacing only the main database would discard committed WAL state and leave
/// the user without the complete damaged file set they were promised.
#[test]
fn confirmed_damage_preserves_the_complete_file_set_before_replacement() {
    let root = test_root("complete-set");
    let files = test_files(&root);
    let expected = [
        b"not a sqlite database".as_slice(),
        b"independent wal bytes".as_slice(),
        b"independent shm bytes".as_slice(),
    ];
    for (path, bytes) in files.iter().zip(expected) {
        write_bytes(path, bytes);
    }
    let recovery_root = root.join("damaged-reports");

    let decision = decide_integrity(
        &files,
        &recovery_root,
        1_785_427_200_000,
        Integrity::Damaged(vec!["fixture corruption".to_string()]),
    );
    let Decision::Recovered(snapshot) = decision else {
        panic!("confirmed corruption must recover, got {decision:?}");
    };

    for (source, bytes) in files.iter().zip(expected) {
        assert!(
            !source.exists(),
            "finalized source must leave its live path"
        );
        let preserved = snapshot.join(source.file_name().unwrap());
        assert_eq!(std::fs::read(preserved).unwrap(), bytes);
        let retired =
            paths::report_recovery_originals_dir(&snapshot).join(source.file_name().unwrap());
        assert_eq!(std::fs::read(retired).unwrap(), bytes);
    }
    let metadata = read_metadata(&snapshot).unwrap();
    assert_eq!(metadata.files.len(), 3);
    assert!(metadata.cause.contains("file synchronization software"));
    assert!(paths::report_recovery_finalized_path(&snapshot).is_file());

    std::fs::remove_dir_all(root).unwrap();
}

/// `db/report_recovery.rs:recent_snapshot` must keep the 24-hour circuit breaker enabled.
///
/// Replacing the age comparison with an unconditional `None` makes the second call recover again,
/// removing the independently supplied current bytes instead of leaving repeated damage untouched.
#[test]
fn recent_recovery_blocks_repeated_damage_without_touching_current_files() {
    let root = test_root("circuit-breaker");
    let files = test_files(&root);
    for (path, bytes) in files.iter().zip([
        b"bad-main".as_slice(),
        b"wal-one".as_slice(),
        b"shm-one".as_slice(),
    ]) {
        write_bytes(path, bytes);
    }
    let recovery_root = root.join("damaged-reports");
    let first_time = 1_785_427_200_000;
    let Decision::Recovered(previous) = decide_integrity(
        &files,
        &recovery_root,
        first_time,
        Integrity::Damaged(vec!["first fixture corruption".to_string()]),
    ) else {
        panic!("first confirmed corruption must recover");
    };

    let current = [
        b"bad-new".as_slice(),
        b"wal-new".as_slice(),
        b"shm-new".as_slice(),
    ];
    for (path, bytes) in files.iter().zip(current) {
        write_bytes(path, bytes);
    }
    let decision = decide_integrity(
        &files,
        &recovery_root,
        first_time + 60 * 60 * 1_000,
        Integrity::Damaged(vec!["second fixture corruption".to_string()]),
    );
    assert!(matches!(
        decision,
        Decision::Blocked {
            snapshot_dir: Some(ref path),
            ..
        } if path == &previous
    ));
    for (path, bytes) in files.iter().zip(current) {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    std::fs::remove_dir_all(root).unwrap();
}

/// `db/report_recovery.rs:resume_pending` must finalize a published snapshot before `NotPresent`.
///
/// Removing the resume call from `prepare_at` returns `Ready` after the simulated partial atomic
/// retirement, leaving stale sidecars beside the fresh database path and losing the crash state.
#[test]
fn published_recovery_resumes_partial_source_removal_after_a_crash() {
    let root = test_root("resume");
    let files = test_files(&root);
    for (path, bytes) in files.iter().zip([
        b"bad-main".as_slice(),
        b"old-wal".as_slice(),
        b"old-shm".as_slice(),
    ]) {
        write_bytes(path, bytes);
    }
    let recovery_root = root.join("damaged-reports");
    let snapshot = publish_pending(&files, &recovery_root, 1_785_427_200_000);
    let retired_main =
        paths::report_recovery_originals_dir(&snapshot).join(files[0].file_name().unwrap());
    std::fs::rename(&files[0], retired_main).unwrap();

    let decision = prepare_at(&files, &recovery_root, 1_785_427_201_000);
    assert_eq!(decision, Decision::Recovered(snapshot.clone()));
    assert!(files.iter().all(|path| !path.exists()));
    assert!(paths::report_recovery_finalized_path(&snapshot).is_file());

    std::fs::remove_dir_all(root).unwrap();
}

/// `db/report_recovery.rs:finalize_snapshot` must re-hash every atomically retired source.
///
/// Deleting the retired-source fingerprint comparison permits a fresh writer to start from a
/// snapshot whose top-level copy never contained the independently changed WAL below.
#[test]
fn changed_source_blocks_pending_finalization_without_deleting_any_payload() {
    let root = test_root("changed-source");
    let files = test_files(&root);
    for (path, bytes) in files.iter().zip([
        b"bad-main".as_slice(),
        b"old-wal".as_slice(),
        b"old-shm".as_slice(),
    ]) {
        write_bytes(path, bytes);
    }
    let recovery_root = root.join("damaged-reports");
    let snapshot = publish_pending(&files, &recovery_root, 1_785_427_200_000);
    write_bytes(&files[1], b"new wal from an external writer");

    let decision = prepare_at(&files, &recovery_root, 1_785_427_201_000);
    assert!(matches!(decision, Decision::Failed(_)));
    let originals = paths::report_recovery_originals_dir(&snapshot);
    let expected = [
        b"bad-main".as_slice(),
        b"new wal from an external writer".as_slice(),
        b"old-shm".as_slice(),
    ];
    for (source, bytes) in files.iter().zip(expected) {
        let retired = originals.join(source.file_name().unwrap());
        let payload = if source.exists() {
            std::fs::read(source).unwrap()
        } else {
            std::fs::read(retired).unwrap()
        };
        assert_eq!(payload, bytes);
    }
    assert!(!paths::report_recovery_finalized_path(&snapshot).exists());

    std::fs::remove_dir_all(root).unwrap();
}

/// `db/report_recovery.rs:acquire_lease` must reject a simultaneous second owner.
///
/// Replacing `BEGIN EXCLUSIVE` with a deferred transaction lets both connections succeed and
/// allows two current MoonTerminal processes to replace or write one portable replica.
#[test]
fn reports_lease_has_exactly_one_live_owner() {
    let root = test_root("lease");
    let path = root.join("reports-recovery-lock.sqlite");
    let first = acquire_lease(&path).unwrap();
    assert!(acquire_lease(&path).is_err());
    drop(first);
    assert!(acquire_lease(&path).is_ok());

    std::fs::remove_dir_all(root).unwrap();
}

/// `db/report_recovery.rs:ensure_access` must reject a process that does not own the lease.
///
/// Replacing its `access_permitted` condition with `true` makes this assertion fail; without the
/// gate, a second current process can open a read-write reader or manually checkpoint the replica.
#[test]
fn report_access_requires_the_process_lease() {
    assert!(ensure_access().is_err());
}

/// Mapping a denied reader to `Other` hides the localized recording-off guidance from Report.
#[test]
fn report_reader_preserves_typed_access_denial() {
    assert!(matches!(
        super::super::open_reader(),
        Err(super::super::ReadFail::Failed {
            kind: super::super::FailKind::ReplicaAccessDenied,
            ..
        })
    ));
}

/// `db/report_recovery.rs:recent_snapshot` must date the breaker from actual finalization.
///
/// Replacing the marker timestamp with `RecoveryMetadata::created_unix_ms` makes the damage below
/// recover again because the simulated crash lasted longer than 24 hours, even though the
/// replacement itself completed only one hour ago.
#[test]
fn resumed_recovery_starts_its_cooldown_when_finalization_completes() {
    let root = test_root("resumed-cooldown");
    let files = test_files(&root);
    for (path, bytes) in files.iter().zip([
        b"bad-main".as_slice(),
        b"old-wal".as_slice(),
        b"old-shm".as_slice(),
    ]) {
        write_bytes(path, bytes);
    }
    let recovery_root = root.join("damaged-reports");
    let started_ms = 1_785_427_200_000;
    let resumed_ms = started_ms + 48 * 60 * 60 * 1_000;
    let snapshot = publish_pending(&files, &recovery_root, started_ms);

    let decision = prepare_at(&files, &recovery_root, resumed_ms);
    assert_eq!(decision, Decision::Recovered(snapshot.clone()));
    assert_eq!(read_finalized_ms(&snapshot).unwrap(), resumed_ms);

    let current = [
        b"bad-new".as_slice(),
        b"wal-new".as_slice(),
        b"shm-new".as_slice(),
    ];
    for (path, bytes) in files.iter().zip(current) {
        write_bytes(path, bytes);
    }
    let decision = decide_integrity(
        &files,
        &recovery_root,
        resumed_ms + 60 * 60 * 1_000,
        Integrity::Damaged(vec!["second fixture corruption".to_string()]),
    );
    assert!(matches!(decision, Decision::Blocked { .. }));
    for (path, bytes) in files.iter().zip(current) {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    std::fs::remove_dir_all(root).unwrap();
}

/// `db/report_recovery.rs:prepare_at` must clean only recognized abandoned staging.
///
/// Removing the cleanup call leaves the independent recognized directory present forever, while
/// the unexpected child proves cleanup remains non-recursive and cannot erase foreign contents.
#[test]
fn preflight_cleans_recognized_staging_and_preserves_unexpected_contents() {
    let _state_guard = integrity::test_state_guard();
    integrity::reset_test_state();
    let root = test_root("staging-cleanup");
    let files = test_files(&root);
    let recovery_root = root.join("damaged-reports");
    std::fs::create_dir_all(&recovery_root).unwrap();

    let removable = recovery_root.join(format!("{STAGING_PREFIX}abandoned"));
    std::fs::create_dir(&removable).unwrap();
    std::fs::create_dir(paths::report_recovery_originals_dir(&removable)).unwrap();
    write_bytes(
        &removable.join(files[0].file_name().unwrap()),
        b"partial copy",
    );

    let retained = recovery_root.join(format!("{STAGING_PREFIX}foreign"));
    std::fs::create_dir(&retained).unwrap();
    write_bytes(&retained.join("unexpected.txt"), b"must survive");

    assert_eq!(
        prepare_at(&files, &recovery_root, 1_785_427_200_000),
        Decision::Ready
    );
    assert!(!removable.exists());
    assert_eq!(
        std::fs::read(retained.join("unexpected.txt")).unwrap(),
        b"must survive"
    );

    std::fs::remove_dir_all(root).unwrap();
    integrity::reset_test_state();
}

/// `db/report_recovery.rs:pending_snapshots` must resume an invalid partial final marker.
///
/// Treating every existing `finalized` file as complete returns `Ready` after the simulated crash,
/// leaving live sidecars beside a missing main path. The recovered marker must instead contain the
/// independent completion timestamp.
#[test]
fn partial_final_marker_is_revalidated_and_atomically_replaced() {
    let root = test_root("partial-marker");
    let files = test_files(&root);
    for (path, bytes) in files.iter().zip([
        b"bad-main".as_slice(),
        b"old-wal".as_slice(),
        b"old-shm".as_slice(),
    ]) {
        write_bytes(path, bytes);
    }
    let recovery_root = root.join("damaged-reports");
    let snapshot = publish_pending(&files, &recovery_root, 1_785_427_200_000);
    let retired_main =
        paths::report_recovery_originals_dir(&snapshot).join(files[0].file_name().unwrap());
    std::fs::rename(&files[0], retired_main).unwrap();
    write_bytes(&paths::report_recovery_finalized_path(&snapshot), b"");

    let finalized_ms = 1_785_427_201_000;
    assert_eq!(
        prepare_at(&files, &recovery_root, finalized_ms),
        Decision::Recovered(snapshot.clone())
    );
    assert!(files.iter().all(|path| !path.exists()));
    assert_eq!(read_finalized_ms(&snapshot).unwrap(), finalized_ms);
    assert!(!paths::report_recovery_finalizing_path(&snapshot).exists());

    std::fs::remove_dir_all(root).unwrap();
}

/// `db/report_recovery.rs:sha256_file` must keep its 1 MiB buffer off the thread stack.
///
/// Replacing the heap-backed buffer with `[0u8; 1024 * 1024]` overflows this 512 KiB thread
/// before the assertion, matching the Windows startup crash before the recovery notice appears.
#[test]
fn report_hashing_fits_the_windows_startup_stack() {
    let root = test_root("hash-stack");
    let path = root.join("payload.bin");
    write_bytes(&path, b"abc");
    let worker_path = path.clone();

    let result = std::thread::Builder::new()
        .name("report-hash-small-stack".to_string())
        .stack_size(512 * 1024)
        .spawn(move || sha256_file(&worker_path))
        .unwrap()
        .join()
        .expect("report hashing must not overflow a 512 KiB stack")
        .unwrap();

    assert_eq!(
        result,
        (
            3,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string()
        )
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// The writer permit must not open the replica before the full-integrity verdict arrives.
///
/// Resolving the gate before `wait` would let schema writes and index builds land on a replica
/// whose index-only damage the cheap gate could not see.
#[test]
fn writer_permit_waits_for_the_full_verdict() {
    let gate = StartupGate::pending();
    let permit = ReportWritePermit {
        gate: gate.clone(),
        anchor: Arc::new(Mutex::new(None)),
    };
    let (allowed_tx, allowed_rx) = std::sync::mpsc::channel();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        allowed_tx.send(permit.wait().0).unwrap();
    });

    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        allowed_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    gate.finish(true);
    assert!(allowed_rx.recv_timeout(Duration::from_secs(1)).unwrap());
    waiter.join().unwrap();
}

/// Writer authorization must require a conclusive full check, not merely the absence of damage.
///
/// Treating a check that could not run as a pass would let schema and index writes reach a
/// replica that was never fully checked.
#[test]
fn inconclusive_full_verdicts_do_not_pass_the_write_gate() {
    assert!(conclusive(&Integrity::Ok));
    assert!(conclusive(&Integrity::NotPresent));
    assert!(!conclusive(&Integrity::CheckFailed("fixture".into())));
}

/// A check that could not run is repeated after each configured pause, then its failure stands.
///
/// Returning the first `CheckFailed` — the behaviour before #1190 — fails the first half: a file
/// an antivirus held for a few seconds after start cost the whole session's recording. Repeating
/// without a bound fails the second: a scan that fails late would re-read the replica forever.
#[test]
fn a_check_that_could_not_run_is_repeated_a_bounded_number_of_times() {
    let failed = || Integrity::CheckFailed("fixture".into());

    let mut verdicts = vec![Integrity::Ok, failed(), failed()];
    let mut pauses = Vec::new();
    assert_eq!(
        full_check(|| verdicts.pop().unwrap(), |pause| pauses.push(pause)),
        Integrity::Ok
    );
    assert_eq!(pauses, CHECK_RETRY_DELAYS);

    let mut runs = 0;
    let mut pauses = Vec::new();
    assert_eq!(
        full_check(
            || {
                runs += 1;
                failed()
            },
            |pause| pauses.push(pause)
        ),
        failed()
    );
    assert_eq!(runs, CHECK_RETRY_DELAYS.len() + 1);
    assert_eq!(pauses, CHECK_RETRY_DELAYS);
}

/// Confirmed damage, a pass and an absent file are final on the first attempt.
///
/// Repeating a `Damaged` verdict would delay the copy-first recovery by the whole retry budget
/// while readers keep opening a file already known to be corrupt.
#[test]
fn a_verdict_that_stands_is_never_repeated() {
    for verdict in [
        Integrity::Ok,
        Integrity::NotPresent,
        Integrity::Damaged(vec!["fixture".into()]),
    ] {
        let mut runs = 0;
        let expected = verdict.clone();
        assert_eq!(
            full_check(
                || {
                    runs += 1;
                    verdict.clone()
                },
                |_| panic!("a final verdict must not be followed by a pause")
            ),
            expected
        );
        assert_eq!(runs, 1);
    }
}

/// The still-running notice appears only once a check has run for the announced time.
///
/// Announcing it from the first second would flash a notice on every start of a small replica;
/// never announcing it leaves a large one recording nothing for minutes with no explanation.
#[test]
fn a_running_check_is_announced_only_after_it_has_run_long() {
    let started = Instant::now();
    assert!(!check_is_slow(None, started + SLOW_CHECK_NOTICE_AFTER));
    assert!(!check_is_slow(Some(started), started));
    assert!(!check_is_slow(
        Some(started),
        started + SLOW_CHECK_NOTICE_AFTER - Duration::from_millis(1)
    ));
    assert!(check_is_slow(
        Some(started),
        started + SLOW_CHECK_NOTICE_AFTER
    ));
}

/// A check that could not run must preserve readable history without permitting writes or
/// implicit checkpoints. A conclusive pass must still allow Report preference writes; confirmed
/// damage closes both. While the check is pending the still-running notice follows its age and
/// goes away with the verdict.
#[test]
fn background_verdict_preserves_read_history_without_bypassing_writes() {
    const CHILD: &str = "MOON_REPORT_GATE_TEST_CASE";
    const ROOT: &str = "MOON_REPORT_GATE_TEST_ROOT";
    let Ok(case) = std::env::var(CHILD) else {
        // Each process owns its own lease and one-shot verdict; no global test reset can model that.
        for case in ["pending", "setup", "healthy", "damage"] {
            let root = test_root(case);
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "db::report_recovery::tests::background_verdict_preserves_read_history_without_bypassing_writes", "--nocapture"])
                .env(CHILD, case)
                .env(ROOT, &root)
                .output()
                .unwrap();
            std::fs::remove_dir_all(root).unwrap();
            assert!(
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
                "{case}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return;
    };
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    assert!(paths::set_data_dir_override(root));
    let path = paths::reports_db_path();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE app_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE marker(value INTEGER); INSERT INTO marker VALUES(42);",
    )
    .unwrap();
    drop(conn);
    assert!(
        LEASE
            .set(Mutex::new(
                acquire_lease(&paths::reports_recovery_lease_path()).unwrap()
            ))
            .is_ok()
    );
    READ_ACCESS_ENABLED.store(true, Ordering::Release);
    // What `prepare` does before it starts the worker; the notice is then read as of a later
    // moment, so the test does not wait two minutes for it.
    let started = Instant::now();
    let later = started + SLOW_CHECK_NOTICE_AFTER;
    assert!(CHECK_STARTED.set(started).is_ok());
    CHECK_PENDING.store(true, Ordering::Release);
    assert_eq!(status_at(started), None);
    assert_eq!(status_at(later), Some(&RecoveryNotice::CheckRunning));
    assert!(write_access_pending());
    if case != "pending" {
        if case == "damage" {
            integrity::record_corruption(&rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(11),
                None,
            ));
        }
        let verdict = match case.as_str() {
            "setup" => Integrity::CheckFailed("fixture setup failure".into()),
            _ => Integrity::Ok,
        };
        assert_eq!(
            finish_background(Decision::Ready, verdict),
            case == "healthy"
        );
        assert_eq!(wait_for_write_access().is_ok(), case == "healthy");
        assert!(!write_access_pending());
        // The verdict replaces the still-running notice: nothing after a pass, the latched
        // recording-off notice after a check that could not run.
        match case.as_str() {
            "healthy" => assert_eq!(status_at(later), None),
            "setup" => assert!(matches!(
                status_at(later),
                Some(RecoveryNotice::CheckIncomplete { .. })
            )),
            _ => assert_ne!(status_at(later), Some(&RecoveryNotice::CheckRunning)),
        }
    }
    let reader = super::super::open_reader();
    if case == "damage" {
        assert!(matches!(
            reader,
            Err(super::super::ReadFail::Failed {
                kind: super::super::FailKind::ReplicaAccessDenied,
                ..
            })
        ));
        return;
    }
    let reader = reader.expect("available history must remain readable");
    assert_eq!(
        reader
            .query_row("SELECT value FROM marker", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        42
    );
    if case == "healthy" {
        super::super::save_sort(&reader, "profitpct", true);
        assert_eq!(
            super::super::load_sort(&reader),
            Some(("profitpct".into(), true))
        );
    } else {
        let error = reader
            .execute("INSERT INTO marker VALUES(99)", [])
            .unwrap_err();
        assert_eq!(
            error.sqlite_error_code(),
            Some(rusqlite::ErrorCode::ReadOnly)
        );
        if case != "pending" {
            assert!(
                status().is_some(),
                "stopped recording must have its own notice"
            );
        }
    }
}

/// Write the three fixture members and return the bytes each one holds.
///
/// Args:
///     files: Main, WAL and SHM paths.
///
/// Returns:
///     The independent expected payload of each member, in the same order.
fn write_fixture_set(files: &[PathBuf; 3]) -> [&'static [u8]; 3] {
    let bytes = [
        b"unchecked main".as_slice(),
        b"unchecked wal".as_slice(),
        b"unchecked shm".as_slice(),
    ];
    for (path, payload) in files.iter().zip(bytes) {
        write_bytes(path, payload);
    }
    bytes
}

/// Names of the entries directly under a recovery root, sorted.
fn root_entries(recovery_root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(recovery_root)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// `set_aside` must move the whole unchecked set into a finalized snapshot that says why.
///
/// Dropping the WAL or SHM from the move leaves a sidecar beside the fresh replica; skipping the
/// finalization marker makes the next start resume an already finished move; recording the
/// damage cause would tell whoever opens the folder that the files were corrupt, which no check
/// established.
#[test]
fn an_unchecked_set_aside_moves_the_whole_set_into_a_finalized_snapshot() {
    let root = test_root("set-aside");
    let files = test_files(&root);
    let bytes = write_fixture_set(&files);
    let recovery_root = root.join("damaged-reports");

    let SetAside::Done(snapshot) = set_aside(
        &files,
        &recovery_root,
        1_785_427_200_000,
        "fixture: file held".into(),
    ) else {
        panic!("an unheld set must be set aside");
    };
    for (source, payload) in files.iter().zip(bytes) {
        assert!(
            !source.exists(),
            "the live path must be free for the fresh replica"
        );
        let retired =
            paths::report_recovery_originals_dir(&snapshot).join(source.file_name().unwrap());
        assert_eq!(std::fs::read(retired).unwrap(), payload);
    }
    assert!(paths::report_recovery_finalized_path(&snapshot).is_file());
    let metadata = read_metadata(&snapshot).unwrap();
    assert!(metadata.cause.contains("No damage was found"));
    assert_eq!(metadata.diagnostics, vec!["fixture: file held".to_string()]);
    assert!(pending_snapshots(&recovery_root).unwrap().is_empty());
    // Not a damage recovery: real damage in the fresh replica the next hour is still recovered.
    assert_eq!(
        recent_snapshot(&recovery_root, 1_785_427_200_000 + 60 * 60 * 1_000).unwrap(),
        None
    );

    std::fs::remove_dir_all(root).unwrap();
}

/// A set-aside that cannot even stage its copy must leave every live file as it was.
///
/// Letting the staging error through after a partial retirement, or removing a source before the
/// copy is verified, fails the byte comparison below.
#[test]
fn a_set_aside_that_cannot_stage_leaves_the_files_in_place() {
    let root = test_root("set-aside-blocked");
    let files = test_files(&root);
    let bytes = write_fixture_set(&files);
    // A file where the recovery directory belongs: creating the staging under it fails.
    let recovery_root = root.join("damaged-reports");
    write_bytes(&recovery_root, b"not a directory");

    let outcome = set_aside(&files, &recovery_root, 1_785_427_200_000, "fixture".into());
    assert!(matches!(outcome, SetAside::Untouched(_)), "got {outcome:?}");
    for (source, payload) in files.iter().zip(bytes) {
        assert_eq!(std::fs::read(source).unwrap(), payload);
    }

    std::fs::remove_dir_all(root).unwrap();
}

/// A retirement that stopped half way must be put back and its published copy withdrawn.
///
/// Without the put-back the main file stays inside the snapshot while the user is told nothing
/// moved. Leaving the copy published hands it to the next start's `resume_pending`, which would
/// retire the live files again for a move the user saw fail.
#[test]
fn a_partly_retired_set_is_put_back_and_its_copy_withdrawn() {
    let root = test_root("set-aside-restore");
    let files = test_files(&root);
    let bytes = write_fixture_set(&files);
    let recovery_root = root.join("damaged-reports");
    let snapshot = stage_and_publish(
        &files,
        &recovery_root,
        1_785_427_200_000,
        FRESH_REPLICA_CAUSE,
        Vec::new(),
    )
    .unwrap();
    // What `finalize_snapshot` leaves when the WAL rename fails after the main one succeeded.
    std::fs::rename(
        &files[0],
        paths::report_recovery_originals_dir(&snapshot).join(files[0].file_name().unwrap()),
    )
    .unwrap();

    assert!(restore_unfinalized(&files, &snapshot).unwrap());
    for (source, payload) in files.iter().zip(bytes) {
        assert_eq!(std::fs::read(source).unwrap(), payload);
    }
    assert!(pending_snapshots(&recovery_root).unwrap().is_empty());
    assert!(
        root_entries(&recovery_root).is_empty(),
        "the unused copy must be removed: {:?}",
        root_entries(&recovery_root)
    );

    std::fs::remove_dir_all(root).unwrap();
}

/// The usual reason a check cannot run is another program holding the file, and SQLite and most
/// scanners open it without delete sharing: the copy then succeeds and the move fails. That must
/// come back as a failure that left the files where they were, not as a half-moved set.
#[cfg(windows)]
#[test]
fn a_main_file_held_without_delete_sharing_is_left_in_place() {
    use std::os::windows::fs::OpenOptionsExt as _;
    /// `FILE_SHARE_READ | FILE_SHARE_WRITE`: what SQLite asks for, so a rename is refused.
    const SHARE_READ_WRITE: u32 = 0x1 | 0x2;

    let root = test_root("set-aside-held");
    let files = test_files(&root);
    let bytes = write_fixture_set(&files);
    let recovery_root = root.join("damaged-reports");
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(SHARE_READ_WRITE)
        .open(&files[0])
        .unwrap();

    let outcome = set_aside(&files, &recovery_root, 1_785_427_200_000, "fixture".into());
    assert!(matches!(outcome, SetAside::Untouched(_)), "got {outcome:?}");
    drop(held);
    for (source, payload) in files.iter().zip(bytes) {
        assert_eq!(std::fs::read(source).unwrap(), payload);
    }
    assert!(root_entries(&recovery_root).is_empty());

    std::fs::remove_dir_all(root).unwrap();
}

/// A published copy that cannot be withdrawn must be reported as stuck, never as untouched.
///
/// Reporting it untouched offers a retry, and the copy left published is one the next start's
/// `resume_pending` finishes by retiring the live files — or, beside a retry's second snapshot,
/// one that makes that start fail closed on two unfinished recoveries.
#[cfg(windows)]
#[test]
fn a_copy_that_cannot_be_withdrawn_stays_published_and_is_reported() {
    use std::os::windows::fs::OpenOptionsExt as _;
    /// `FILE_SHARE_READ | FILE_SHARE_WRITE`: a scanner holding the copy refuses the rename.
    const SHARE_READ_WRITE: u32 = 0x1 | 0x2;

    let root = test_root("set-aside-withdraw");
    let files = test_files(&root);
    let bytes = write_fixture_set(&files);
    let recovery_root = root.join("damaged-reports");
    let snapshot = stage_and_publish(
        &files,
        &recovery_root,
        1_785_427_200_000,
        FRESH_REPLICA_CAUSE,
        Vec::new(),
    )
    .unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(SHARE_READ_WRITE)
        .open(snapshot.join(files[0].file_name().unwrap()))
        .unwrap();

    assert!(!restore_unfinalized(&files, &snapshot).unwrap());
    drop(held);
    for (source, payload) in files.iter().zip(bytes) {
        assert_eq!(std::fs::read(source).unwrap(), payload);
    }
    assert_eq!(pending_snapshots(&recovery_root).unwrap(), vec![snapshot]);

    std::fs::remove_dir_all(root).unwrap();
}

/// Wait for a condition another thread makes true, failing the test after ten seconds.
///
/// Args:
///     what: Named in the failure.
///     done: The condition, asked every 10 ms.
fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// After a check that could not run, the real writer must wait for the user's fresh replica and
/// record into it, and a set-aside that fails must leave history readable and the action offered.
///
/// Breaks on: the writer exiting on the closed gate as before (nothing ever offers the action and
/// `ForgetCore` is never answered); the writer finishing its initialization before it parks (the
/// feeds would declare catch-up into a replica nobody writes); a second press starting a second
/// move; a failed move closing reads for good or offering nothing to retry; and treating the
/// check that could not run as a pass (writes open before the move).
#[test]
fn an_unchecked_replica_is_set_aside_and_the_parked_writer_records_into_a_fresh_one() {
    const CHILD: &str = "MOON_REPORT_FRESH_TEST_ROOT";
    let Some(root) = std::env::var_os(CHILD) else {
        // Process-global gates, lease and notices: one process per run, as in the test above.
        let root = test_root("fresh-replica");
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "db::report_recovery::tests::an_unchecked_replica_is_set_aside_and_the_parked_writer_records_into_a_fresh_one", "--nocapture"])
            .env(CHILD, &root)
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(root);
        assert!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    };
    assert!(paths::set_data_dir_override(PathBuf::from(root)));
    let path = paths::reports_db_path();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE app_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE marker(value INTEGER); INSERT INTO marker VALUES(42);",
    )
    .unwrap();
    drop(conn);
    assert!(
        LEASE
            .set(Mutex::new(
                acquire_lease(&paths::reports_recovery_lease_path()).unwrap()
            ))
            .is_ok()
    );
    // What `prepare` sets up before its worker: readable history, a pending gate, a running check.
    READ_ACCESS_ENABLED.store(true, Ordering::Release);
    assert!(CHECK_STARTED.set(Instant::now()).is_ok());
    CHECK_PENDING.store(true, Ordering::Release);
    let anchor = Arc::new(Mutex::new(startup_read_anchor(&path).unwrap()));
    let gate = StartupGate::pending();
    let handle = super::super::spawn_writer(ReportWritePermit {
        gate: gate.clone(),
        anchor: anchor.clone(),
    })
    .expect("writer thread");
    assert!(
        !fresh_replica_offered(),
        "never offered while the check runs"
    );
    assert!(!begin_fresh_replica());
    wait_until(
        "the writer to wait for the verdict",
        fresh_replica_offer_pending,
    );

    // The verdict the worker publishes after its last attempt, then the permit it resolves.
    assert!(!finish_background(
        Decision::Ready,
        Integrity::CheckFailed("fixture: file held".into())
    ));
    let incomplete = status().expect("the incomplete-check notice");
    gate.finish(false);
    wait_until("the writer to park", fresh_replica_offered);
    assert!(
        !fresh_replica_offer_pending(),
        "a surface waiting for the writer's decision would poll forever"
    );
    // Same words, new address: the UI compares notices by identity, and a surface that
    // rendered before the writer parked must be told the button is there now.
    let offered = status().expect("the offered notice");
    assert_eq!(offered, incomplete);
    assert!(!std::ptr::eq(offered, incomplete));
    assert!(
        anchor.lock().unwrap().is_none(),
        "the anchor would block the move"
    );
    assert!(
        !handle.tx.is_started(),
        "feeds must keep their catch-up undeclared"
    );
    assert!(recording_stopped());
    // A message sent meanwhile is held, not refused, and must not reach the fresh replica.
    handle
        .tx
        .send(super::super::DbMsg::ValuationAck { through_seq: 7 });

    // The move fails: the recovery directory cannot be created.
    let blocker = paths::damaged_reports_dir();
    std::fs::write(&blocker, b"not a directory").unwrap();
    assert!(begin_fresh_replica());
    assert!(
        !begin_fresh_replica(),
        "a second press must not start a second move"
    );
    assert!(!fresh_replica_offered());
    assert_eq!(status(), Some(&RecoveryNotice::FreshReplicaRunning));
    run_fresh_replica();
    assert_eq!(status(), Some(&RecoveryNotice::FreshReplicaFailed));
    assert!(
        fresh_replica_offered(),
        "a failed move must be offered again"
    );
    assert!(wait_for_write_access().is_err());
    let reader = super::super::open_reader().expect("history stays readable after a failed move");
    assert_eq!(
        reader
            .query_row("SELECT value FROM marker", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        42
    );
    drop(reader);

    // The cause is gone: the real thread moves the set and releases the writer.
    std::fs::remove_file(&blocker).unwrap();
    assert!(start_fresh_replica());
    wait_until("the fresh writer", || handle.tx.is_started());
    let Some(RecoveryNotice::FreshReplicaStarted { snapshot_dir }) = status() else {
        panic!("expected a started fresh replica, got {:?}", status());
    };
    let retired = paths::report_recovery_originals_dir(snapshot_dir).join("reports.sqlite");
    assert!(retired.is_file(), "the old main file must be kept");
    assert!(!fresh_replica_offered());
    assert!(!recording_stopped());
    assert!(wait_for_write_access().is_ok());
    // The writer answers on the fresh replica, which no longer holds the old rows.
    let (done, answered) = std::sync::mpsc::sync_channel(1);
    handle.tx.send(super::super::DbMsg::ForgetCore {
        core_uid: 7,
        resync: None,
        forget: false,
        done,
    });
    assert_eq!(answered.recv_timeout(Duration::from_secs(10)), Ok(true));
    let reader = super::super::open_reader().expect("the fresh replica is readable");
    assert!(
        reader
            .query_row("SELECT value FROM marker", [], |row| row.get::<_, i64>(0))
            .is_err()
    );
    // The held acknowledgement numbers the fresh outbox above it, so the valuation worker, which
    // still remembers 7, does not skip the fresh replica's first events as already reflected.
    let next_seq: i64 = reader
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name = 'valuation_outbox'",
            [],
            |row| row.get(0),
        )
        .expect("the fresh outbox sequence was seeded");
    assert!(next_seq >= 7, "fresh outbox would restart at {next_seq}");
}
