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
