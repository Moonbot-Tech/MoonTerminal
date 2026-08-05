//! Daily settings-backup regression tests.

use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

/// Test-only sequence for isolated temporary roots.
static TEST_SEQ: AtomicU32 = AtomicU32::new(0);

/// Create a unique temporary root without an extra test dependency.
fn temp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "moonterminal-settings-backup-{}-{tag}-{}",
        std::process::id(),
        TEST_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// Write one source after creating its parent.
fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

/// Create one completed settings snapshot.
fn create_snapshot(backups: &Path, name: &str) {
    let directory = backups.join(name);
    std::fs::create_dir_all(&directory).unwrap();
    write(&directory.join("settings.toml"), name);
    std::fs::write(directory.join(COMPLETION_NAME), COMPLETION_CONTENT).unwrap();
}

/// Naming the slot from process launch instead of UTC noon would create repeated catch-up copies.
#[test]
fn a_late_start_fills_the_canonical_noon_slot_once() {
    let root = temp_root("catch-up");
    let settings = root.join("settings.toml");
    let servers = root.join("servers.enc");
    let backups = root.join("backups");
    write(&settings, "settings-v1");
    write(&servers, "servers-v1");
    let noon = 20_000 * DAY_MS + 12 * 60 * 60 * 1_000;

    let first = backup_due_into(&[&servers, &settings], &backups, noon + 3_600_000).unwrap();
    let second = backup_due_into(&[&servers, &settings], &backups, noon + 3_601_000).unwrap();

    let destination = backups.join(utc_stamp_compact(noon));
    assert_eq!(first, DueOutcome::Created(destination.clone()));
    assert_eq!(second, DueOutcome::Current(destination.clone()));
    assert_eq!(
        std::fs::read_to_string(destination.join("settings.toml")).unwrap(),
        "settings-v1"
    );
    assert_eq!(
        std::fs::read_to_string(destination.join("servers.enc")).unwrap(),
        "servers-v1"
    );
    assert!(destination.join(COMPLETION_NAME).is_file());
    let _ = std::fs::remove_dir_all(root);
}

/// Treating absent sources as a completed day would permanently miss a first-launch backup.
#[test]
fn absent_sources_leave_the_daily_slot_pending() {
    let root = temp_root("missing");
    let backups = root.join("backups");

    let outcome = backup_due_into(&[&root.join("settings.toml")], &backups, 0).unwrap();

    assert_eq!(outcome, DueOutcome::SourceMissing);
    assert!(!backups.exists());
    let _ = std::fs::remove_dir_all(root);
}

/// Publishing when only one pair member exists would make a transient sync gap permanent for the
/// whole UTC period.
#[test]
fn one_missing_pair_member_leaves_the_daily_slot_pending() {
    let root = temp_root("partial-missing");
    let settings = root.join("settings.toml");
    let servers = root.join("servers.enc");
    let backups = root.join("backups");
    write(&settings, "settings-v1");

    let outcome = backup_due_into(&[&servers, &settings], &backups, 0).unwrap();

    assert_eq!(outcome, DueOutcome::SourceMissing);
    assert!(!backups.exists());
    let _ = std::fs::remove_dir_all(root);
}

/// Silently skipping a non-file source would publish a torn config pair without its API data.
#[test]
fn a_non_file_source_rejects_the_entire_snapshot() {
    let root = temp_root("invalid-source");
    let settings = root.join("settings.toml");
    let servers = root.join("servers.enc");
    let backups = root.join("backups");
    write(&settings, "settings-v1");
    std::fs::create_dir_all(&servers).unwrap();

    let result = backup_due_into(&[&servers, &settings], &backups, 0);

    assert!(result.is_err());
    assert!(!backups.exists());
    let _ = std::fs::remove_dir_all(root);
}

/// Using an eight-day cutoff or count-based pruning would violate seven-period retention.
#[test]
fn retention_keeps_every_snapshot_in_the_latest_seven_periods() {
    let root = temp_root("retention");
    let backups = root.join("backups");
    let current = 20_000 * DAY_MS + 12 * 60 * 60 * 1_000;
    for age in 0..=7 {
        create_snapshot(&backups, &utc_stamp_compact(current - age * DAY_MS));
    }
    let expected = ["settings.toml", COMPLETION_NAME];

    let removed = prune(&backups, current, &["settings.toml"], &expected);

    assert_eq!(removed, 1);
    assert!(!backups
        .join(utc_stamp_compact(current - 7 * DAY_MS))
        .exists());
    assert!(backups
        .join(utc_stamp_compact(current - 6 * DAY_MS))
        .exists());
    let _ = std::fs::remove_dir_all(root);
}

/// Deleting a timestamp-looking directory before validating ownership could destroy user files.
#[test]
fn retention_preserves_a_snapshot_with_foreign_content() {
    let root = temp_root("foreign");
    let backups = root.join("backups");
    let current = 20_000 * DAY_MS + 12 * 60 * 60 * 1_000;
    let old = utc_stamp_compact(current - 7 * DAY_MS);
    create_snapshot(&backups, &old);
    write(&backups.join(&old).join("notes.txt"), "keep");
    let expected = ["settings.toml", COMPLETION_NAME];

    assert_eq!(prune(&backups, current, &["settings.toml"], &expected), 0);
    assert!(backups.join(old).join("notes.txt").exists());
    let _ = std::fs::remove_dir_all(root);
}
