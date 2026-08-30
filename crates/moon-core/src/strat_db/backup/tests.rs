//! Regression tests for scheduled strategy backup timing, publication, and retention.

use std::sync::atomic::{AtomicU32, Ordering};

use rusqlite::Connection;

use super::*;
use crate::backups::NOON_MS;

/// Test-only sequence for isolated temporary roots.
static TEST_SEQ: AtomicU32 = AtomicU32::new(0);

/// Create a unique temporary directory without adding a test-only dependency.
fn temp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "moonterminal-strategy-backup-{}-{tag}-{}",
        std::process::id(),
        TEST_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// Create a minimal live SQLite strategy database.
fn create_database(path: &Path) {
    let connection = Connection::open(path).unwrap();
    connection
        .execute("CREATE TABLE strategies (name TEXT NOT NULL)", [])
        .unwrap();
    connection
        .execute("INSERT INTO strategies VALUES ('alpha')", [])
        .unwrap();
}

/// Treating a schema-only database as inherited data would recreate the observed empty snapshot.
#[test]
fn inherited_readiness_requires_a_durable_strategy_row() {
    let root = temp_root("row-probe");
    let source = root.join("strategies.sqlite");
    let connection = Connection::open(&source).unwrap();
    connection
        .execute("CREATE TABLE strategies (name TEXT NOT NULL)", [])
        .unwrap();
    drop(connection);
    assert!(!source_has_strategy_rows_at(&source));

    let connection = Connection::open(&source).unwrap();
    connection
        .execute("INSERT INTO strategies VALUES ('alpha')", [])
        .unwrap();
    drop(connection);
    assert!(source_has_strategy_rows_at(&source));
    let _ = std::fs::remove_dir_all(root);
}

/// Create one completed-looking snapshot directory for retention tests.
fn create_snapshot_dir(backups: &Path, name: &str) {
    let dir = backups.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(DATABASE_NAME), b"SQLite format 3\0").unwrap();
    std::fs::write(dir.join(COMPLETION_NAME), COMPLETION_CONTENT).unwrap();
}

/// Publish a test snapshot without production-global topology state.
fn publish_current(
    _generation: u64,
    store: &SnapshotStore<'_>,
    staging: &Path,
    stamp: &str,
) -> anyhow::Result<Option<ExactPublication>> {
    store
        .publish_exact(staging, stamp, is_completed_snapshot)
        .map(Some)
}

/// Reject a test snapshot as belonging to a stale topology.
fn publish_stale(
    _generation: u64,
    _store: &SnapshotStore<'_>,
    _staging: &Path,
    _stamp: &str,
) -> anyhow::Result<Option<ExactPublication>> {
    Ok(None)
}

/// Replacing the due slot with `now_ms` would publish catch-up under the launch time and fail to
/// mark the missed noon slot complete, causing another backup on every five-minute retry.
#[test]
fn a_late_start_publishes_the_missing_noon_slot_once() {
    let root = temp_root("catch-up");
    let src = root.join("strategies.sqlite");
    let backups = root.join("backups");
    create_database(&src);
    let day = 20_000 * DAY_MS;
    let now = day + NOON_MS + 3 * 60 * 60 * 1_000;

    let first = backup_due_into(&src, &backups, now, 1, publish_current).unwrap();
    let second = backup_due_into(&src, &backups, now + 1_000, 1, publish_current).unwrap();

    assert!(matches!(first, DueOutcome::Created(_)));
    assert_eq!(
        second,
        DueOutcome::Current(backups.join(utc_stamp_compact(day + NOON_MS)))
    );
    let copied = Connection::open_with_flags(
        backups
            .join(utc_stamp_compact(day + NOON_MS))
            .join(DATABASE_NAME),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let name: String = copied
        .query_row("SELECT name FROM strategies", [], |row| row.get(0))
        .unwrap();
    assert_eq!(name, "alpha");
    let _ = std::fs::remove_dir_all(root);
}

/// Treating a missing lazy database as success would sleep until tomorrow and permanently miss
/// today's backup when the first strategy snapshot creates the database minutes later.
#[test]
fn an_absent_lazy_database_keeps_the_due_slot_pending() {
    let root = temp_root("missing");
    let backups = root.join("backups");

    let outcome = backup_due_into(
        &root.join("strategies.sqlite"),
        &backups,
        20_000 * DAY_MS,
        1,
        publish_current,
    )
    .unwrap();

    assert_eq!(outcome, DueOutcome::SourceMissing);
    assert!(!backups.exists());
    let _ = std::fs::remove_dir_all(root);
}

/// Publishing after topology changed could omit a newly enabled core from the canonical day.
#[test]
fn a_stale_topology_discards_its_assembled_snapshot() {
    let root = temp_root("stale-topology");
    let source = root.join("strategies.sqlite");
    let backups = root.join("backups");
    create_database(&source);

    let outcome = backup_due_into(&source, &backups, 0, 7, publish_stale).unwrap();

    assert_eq!(outcome, DueOutcome::Pending);
    assert!(
        std::fs::read_dir(&backups)
            .map(|entries| entries.count() == 0)
            .unwrap_or(true),
        "a stale assembly must publish neither a slot nor staging debris"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Changing the cutoff from `RETAIN_PERIODS - 1` to `RETAIN_PERIODS` would retain eight UTC
/// periods, while pruning by count would incorrectly delete one of multiple manual snapshots in
/// a retained day.
#[test]
fn retention_keeps_every_snapshot_in_the_latest_seven_utc_periods() {
    let root = temp_root("retention");
    let backups = root.join("backups");
    std::fs::create_dir_all(&backups).unwrap();
    let current = 20_000 * DAY_MS + NOON_MS;
    for age in 0..=7 {
        create_snapshot_dir(&backups, &utc_stamp_compact(current - age * DAY_MS));
    }
    let retained_manual = utc_stamp_compact(current - 6 * DAY_MS + 60_000);
    create_snapshot_dir(&backups, &retained_manual);
    std::fs::create_dir_all(backups.join("2026-08-04")).unwrap();

    let removed = prune(&backups, current);

    assert_eq!(removed, 1);
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
    assert!(backups.join(retained_manual).exists());
    assert!(backups.join("2026-08-04").exists());
    let _ = std::fs::remove_dir_all(root);
}

/// Replacing `SnapshotStore::publish_distinct`'s suffix loop with one rename to `base` would make
/// the second manual click fail instead of preserving both on-demand recovery points.
#[test]
fn manual_publication_uses_a_collision_suffix() {
    let root = temp_root("manual-collision");
    let src = root.join("strategies.sqlite");
    let backups = root.join("backups");
    create_database(&src);
    let stamp = "20260804-150000-manual";

    let first = create_distinct_snapshot(&src, &backups, stamp).unwrap();
    let second = create_distinct_snapshot(&src, &backups, stamp).unwrap();

    assert_eq!(
        first.file_name().and_then(|name| name.to_str()),
        Some(stamp)
    );
    assert_eq!(
        second.file_name().and_then(|name| name.to_str()),
        Some("20260804-150000-manual-01")
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Removing the `-manual` namespace would let a click during the exact noon second occupy the
/// canonical scheduled name, so the worker would report `Current` instead of taking its own copy.
#[test]
fn a_manual_backup_at_noon_cannot_replace_the_scheduled_slot() {
    let root = temp_root("manual-noon");
    let src = root.join("strategies.sqlite");
    let backups = root.join("backups");
    create_database(&src);
    let noon = 20_000 * DAY_MS + NOON_MS;

    let manual = backup_manual_at(&src, &backups, noon).unwrap();
    let scheduled = backup_due_into(&src, &backups, noon, 1, publish_current).unwrap();

    assert_eq!(
        manual.file_name().and_then(|name| name.to_str()),
        Some("20241004-120000-manual")
    );
    assert_eq!(
        scheduled,
        DueOutcome::Created(backups.join("20241004-120000"))
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Deleting expected files before checking the directory would destroy a usable old snapshot when
/// a cloud client or user had placed one unrelated file beside it.
#[test]
fn an_unexpected_file_preserves_the_entire_snapshot() {
    let root = temp_root("foreign-file");
    let backups = root.join("backups");
    let current = 20_000 * DAY_MS + NOON_MS;
    let old = utc_stamp_compact(current - 7 * DAY_MS);
    create_snapshot_dir(&backups, &old);
    std::fs::write(backups.join(&old).join("notes.txt"), b"keep").unwrap();

    assert_eq!(prune(&backups, current), 0);
    assert!(backups.join(&old).join(DATABASE_NAME).exists());
    assert!(backups.join(&old).join(COMPLETION_NAME).exists());
    assert!(backups.join(&old).join("notes.txt").exists());
    let _ = std::fs::remove_dir_all(root);
}

/// Removing the completion-marker or SQLite-header checks would let a foreign lookalike suppress
/// the real daily copy by being accepted as `Current` under the canonical noon name.
#[test]
fn a_foreign_lookalike_cannot_satisfy_the_scheduled_slot() {
    let root = temp_root("lookalike");
    let src = root.join("strategies.sqlite");
    let backups = root.join("backups");
    create_database(&src);
    let noon = 20_000 * DAY_MS + NOON_MS;
    let occupied = backups.join(utc_stamp_compact(noon));
    std::fs::create_dir_all(&occupied).unwrap();
    std::fs::write(occupied.join(DATABASE_NAME), b"not sqlite at all").unwrap();
    std::fs::write(occupied.join(COMPLETION_NAME), COMPLETION_CONTENT).unwrap();

    let error = backup_due_into(&src, &backups, noon, 1, publish_current).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("occupied by an incomplete or foreign entry"),
        "unexpected error: {error:#}"
    );
    let _ = std::fs::remove_dir_all(root);
}
