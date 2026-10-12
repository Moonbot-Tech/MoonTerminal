//! Regression tests for order-trace snapshots: live-writer copy, publication, retention, catch-up.

use std::sync::atomic::{AtomicU32, Ordering};

use rusqlite::Connection;

use super::*;
use crate::backups::NOON_MS;

/// Test-only sequence for isolated temporary roots.
static TEST_SEQ: AtomicU32 = AtomicU32::new(0);

/// Create a unique temporary directory without adding a test-only dependency.
fn temp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "moonterminal-traces-backup-{}-{tag}-{}",
        std::process::id(),
        TEST_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// Open a WAL-mode writer that never checkpoints on its own, holding one committed answer.
fn open_writer(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
    conn.execute_batch(
        "CREATE TABLE trace_answers (report_uid INTEGER PRIMARY KEY, line_count INTEGER NOT NULL);
         INSERT INTO trace_answers VALUES (1, 3);",
    )
    .unwrap();
    conn
}

/// Return the answered report uids stored in a snapshot's database, ascending.
fn snapshot_uids(dir: &Path) -> Vec<i64> {
    let conn = Connection::open_with_flags(
        dir.join(DATABASE_NAME),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut stmt = conn
        .prepare("SELECT report_uid FROM trace_answers ORDER BY report_uid")
        .unwrap();
    stmt.query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Create one completed-looking snapshot directory for retention tests.
fn create_snapshot_dir(backups: &Path, name: &str) {
    let dir = backups.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(DATABASE_NAME), b"SQLite format 3\0").unwrap();
    std::fs::write(dir.join(COMPLETION_NAME), COMPLETION_CONTENT).unwrap();
}

/// Replacing `VACUUM INTO` with a raw `fs::copy` of the main file would lose every answer still
/// sitting in the WAL, and copying mid-transaction would capture a row the writer never committed.
#[test]
fn a_snapshot_under_a_live_writer_holds_exactly_the_committed_rows() {
    let root = temp_root("live-writer");
    let src = root.join(DATABASE_NAME);
    let backups = root.join("backups");
    let writer = open_writer(&src);
    writer
        .execute("INSERT INTO trace_answers VALUES (2, 0)", [])
        .unwrap();
    let wal = root.join(format!("{DATABASE_NAME}-wal"));
    assert!(
        std::fs::metadata(&wal).unwrap().len() > 0,
        "committed rows must still be in the WAL for this test to mean anything"
    );
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    writer
        .execute("INSERT INTO trace_answers VALUES (3, 1)", [])
        .unwrap();

    let published = backup_manual_at(&src, &backups, 20_000 * DAY_MS + NOON_MS)
        .unwrap()
        .expect("an existing archive must produce a snapshot");

    assert_eq!(snapshot_uids(&published), vec![1, 2]);
    writer.execute_batch("COMMIT").unwrap();
    assert!(is_completed_snapshot(&published));
    drop(writer);
    let _ = std::fs::remove_dir_all(root);
}

/// Dropping the marker write or writing it outside the staged directory would publish a snapshot
/// the validator rejects, and leaving staging behind would grow `backups/` on every run.
#[test]
fn publication_renames_one_marked_directory_and_leaves_no_staging() {
    let root = temp_root("publish");
    let src = root.join(DATABASE_NAME);
    let backups = root.join("backups");
    drop(open_writer(&src));
    let now = 20_000 * DAY_MS + NOON_MS + 60_000;

    let first = backup_manual_at(&src, &backups, now).unwrap().unwrap();
    let second = backup_manual_at(&src, &backups, now).unwrap().unwrap();

    assert_eq!(
        std::fs::read(first.join(COMPLETION_NAME)).unwrap(),
        COMPLETION_CONTENT
    );
    assert_ne!(first, second, "a second click in one second keeps both");
    let leftovers: Vec<_> = std::fs::read_dir(&backups)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "staging left behind: {leftovers:?}");
    let _ = std::fs::remove_dir_all(root);
}

/// A strategy marker must not satisfy this domain: accepting a foreign versioned marker would let
/// a copied strategy snapshot stand in for a missing order-trace slot.
#[test]
fn a_foreign_marker_does_not_complete_a_snapshot() {
    let root = temp_root("foreign");
    let dir = root.join("snap");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(DATABASE_NAME), b"SQLite format 3\0").unwrap();
    std::fs::write(
        dir.join(COMPLETION_NAME),
        b"moonterminal-strategy-backup-v1\n",
    )
    .unwrap();

    assert!(!is_completed_snapshot(&dir));
    let _ = std::fs::remove_dir_all(root);
}

/// Changing the cutoff from `RETAIN_PERIODS - 1` to `RETAIN_PERIODS` would keep eight periods,
/// and pruning a non-grammar directory would delete a user's own folder.
#[test]
fn retention_keeps_the_latest_seven_utc_periods() {
    let root = temp_root("retention");
    let backups = root.join("backups");
    std::fs::create_dir_all(&backups).unwrap();
    let current = 20_000 * DAY_MS + NOON_MS;
    for age in 0..=7 {
        create_snapshot_dir(&backups, &utc_stamp_compact(current - age * DAY_MS));
    }
    std::fs::create_dir_all(backups.join("keep-me")).unwrap();

    assert_eq!(prune(&backups, current), 1);
    assert!(
        !backups
            .join(utc_stamp_compact(current - 7 * DAY_MS))
            .exists()
    );
    assert!(
        backups
            .join(utc_stamp_compact(current - 6 * DAY_MS))
            .exists()
    );
    assert!(backups.join("keep-me").exists());
    let _ = std::fs::remove_dir_all(root);
}

/// Publishing catch-up under `now_ms` instead of the due slot would repeat the backup on every
/// retry; treating an absent archive as done would skip the day once the writer creates it.
#[test]
fn catch_up_publishes_the_missed_noon_slot_once_and_waits_for_the_file() {
    let root = temp_root("catch-up");
    let src = root.join(DATABASE_NAME);
    let backups = root.join("backups");
    let day = 20_000 * DAY_MS;
    let now = day + NOON_MS - 60_000;
    let due = backups.join(utc_stamp_compact(day + NOON_MS - DAY_MS));

    assert_eq!(
        backup_due_into(&src, &backups, now).unwrap(),
        DueOutcome::SourceMissing
    );
    drop(open_writer(&src));
    let first = backup_due_into(&src, &backups, now).unwrap();
    let second = backup_due_into(&src, &backups, now + 1_000).unwrap();

    assert_eq!(first, DueOutcome::Created(due.clone()));
    assert_eq!(second, DueOutcome::Current(due.clone()));
    assert_eq!(snapshot_uids(&due), vec![1]);
    let _ = std::fs::remove_dir_all(root);
}

/// Without the manual-path presence check, a click before any trace was archived would surface a
/// "file not found" error instead of quietly skipping a domain that has nothing to save yet.
#[test]
fn a_manual_backup_without_an_archive_is_a_no_op() {
    let root = temp_root("manual-missing");
    let backups = root.join("backups");

    assert_eq!(
        backup_manual_at(&root.join(DATABASE_NAME), &backups, 20_000 * DAY_MS).unwrap(),
        None
    );
    assert!(!backups.exists());
    let _ = std::fs::remove_dir_all(root);
}
