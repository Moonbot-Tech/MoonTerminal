//! Safety and policy tests for the shared snapshot-directory lifecycle.

use super::*;

/// Create an isolated directory for one filesystem test.
fn temp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "moonterminal-snapshot-store-{}-{tag}-{}",
        std::process::id(),
        STAGING_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// Create a completed snapshot with the supplied regular files.
fn snapshot(root: &Path, name: &str, files: &[&str]) -> PathBuf {
    let directory = root.join(name);
    std::fs::create_dir_all(&directory).unwrap();
    for file in files {
        std::fs::write(directory.join(file), b"data").unwrap();
    }
    directory
}

/// Replacing create-only claims with remove-then-create would delete the first live staging
/// directory when a second config/manual operation starts in the same process.
#[test]
fn concurrent_staging_claims_are_distinct_and_remain_live() {
    let root = temp_root("staging");
    let backups = root.join("backups");
    let store = SnapshotStore::new(&backups, &["data"], true);

    let first = store.create_staging().unwrap();
    std::fs::write(first.join("data"), b"copy in progress").unwrap();
    let second = store.create_staging().unwrap();

    assert_ne!(first, second);
    assert!(first.join("data").exists());
    assert!(second.exists());
    let _ = std::fs::remove_dir_all(root);
}

/// Removing the 24-hour activity gate would classify a live staging directory as crash litter.
#[test]
fn recent_staging_is_not_stale_but_old_activity_is() {
    let root = temp_root("stale");
    let staging = root.join(".incoming-1-1");
    std::fs::create_dir(&staging).unwrap();
    let now = SystemTime::now();

    assert!(!staging_is_stale(&staging, now));
    assert!(staging_is_stale(
        &staging,
        now + STAGING_STALE_AFTER + Duration::from_secs(1)
    ));
    let _ = std::fs::remove_dir_all(root);
}

/// Weakening staging ownership to `starts_with(".incoming-")` would recursively delete a user's
/// similarly named directory after the inactivity threshold.
#[test]
fn staging_names_require_numeric_process_and_sequence_components() {
    assert!(is_staging_name(".incoming-123-456"));
    assert!(!is_staging_name(".incoming-not-ours"));
    assert!(!is_staging_name(".incoming-123-456-extra"));
    assert!(!is_staging_name(".incoming-123-"));
}

/// Reusing an occupied base name would overwrite a same-second config/manual recovery point.
#[test]
fn distinct_publication_uses_a_collision_suffix() {
    let root = temp_root("distinct");
    let backups = root.join("backups");
    let store = SnapshotStore::new(&backups, &["data"], true);
    let first = store.create_staging().unwrap();
    std::fs::write(first.join("data"), b"one").unwrap();
    let second = store.create_staging().unwrap();
    std::fs::write(second.join("data"), b"two").unwrap();

    assert_eq!(
        store.publish_distinct(&first, "20260804-120000").unwrap(),
        backups.join("20260804-120000")
    );
    assert_eq!(
        store.publish_distinct(&second, "20260804-120000").unwrap(),
        backups.join("20260804-120000-01")
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Accepting an exact-name winner without its domain validator would let a foreign lookalike
/// suppress a scheduled strategy snapshot.
#[test]
fn exact_publication_accepts_only_a_domain_validated_winner() {
    let root = temp_root("exact");
    let backups = root.join("backups");
    let store = SnapshotStore::new(&backups, &["data"], true);
    let existing = snapshot(&backups, "20260804-120000", &["data"]);
    let invalid_staging = store.create_staging().unwrap();
    std::fs::write(invalid_staging.join("data"), b"new").unwrap();

    assert!(
        store
            .publish_exact(&invalid_staging, "20260804-120000", |_| false)
            .is_err()
    );
    assert!(existing.exists());

    let valid_staging = store.create_staging().unwrap();
    std::fs::write(valid_staging.join("data"), b"new").unwrap();
    assert_eq!(
        store
            .publish_exact(&valid_staging, "20260804-120000", |path| {
                path.join("data").exists()
            })
            .unwrap(),
        ExactPublication::Existing(existing)
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Weakening a domain's complete-name grammar to timestamp-prefix-only would delete the user's
/// `-user` directory even though the shared timestamp parser recognizes its ordering prefix.
#[test]
fn retention_uses_the_domains_complete_name_grammar() {
    let root = temp_root("grammar");
    let backups = root.join("backups");
    let store = SnapshotStore::new(&backups, &["data"], true);
    snapshot(&backups, "20260801-120000", &["data"]);
    snapshot(&backups, "20260801-120000-user", &["data"]);

    let removed = store.prune_where(
        |name| name.len() == 15,
        |name| timestamp_prefix(name).is_some_and(|stamp| stamp < "20260802-120000"),
        |_| true,
    );

    assert_eq!(removed, 1);
    assert!(backups.join("20260801-120000-user").exists());
    let _ = std::fs::remove_dir_all(root);
}

/// Removing the whole-entry preflight would delete owned files before discovering a foreign file.
#[test]
fn a_foreign_entry_preserves_the_whole_snapshot() {
    let root = temp_root("foreign");
    let backups = root.join("backups");
    let store = SnapshotStore::new(&backups, &["data"], true);
    let directory = snapshot(&backups, "20260801-120000", &["data", "notes"]);

    assert_eq!(store.prune_where(|_| true, |_| true, |_| true), 0);
    assert!(directory.join("data").exists());
    assert!(directory.join("notes").exists());
    let _ = std::fs::remove_dir_all(root);
}

/// Treating optional config sources like required strategy files would make one-file first-launch
/// config snapshots immortal, while making strategy files optional would delete partial snapshots.
#[test]
fn required_and_optional_expected_sets_have_distinct_removal_rules() {
    let root = temp_root("required");
    let optional_root = root.join("optional");
    snapshot(&optional_root, "20260801-120000", &["settings.toml"]);
    let optional = SnapshotStore::new(&optional_root, &["settings.toml", "servers.enc"], false);
    assert_eq!(optional.prune_where(|_| true, |_| true, |_| true), 1);

    let required_root = root.join("required");
    let partial = snapshot(&required_root, "20260801-120000", &["strategies.sqlite"]);
    let required = SnapshotStore::new(&required_root, &["strategies.sqlite", ".complete"], true);
    assert_eq!(required.prune_where(|_| true, |_| true, |_| true), 0);
    assert!(partial.join("strategies.sqlite").exists());
    let _ = std::fs::remove_dir_all(root);
}

/// Replacing the shared backup parent with a regular file must fail before any staging write.
#[test]
fn a_non_directory_backup_parent_is_rejected() {
    let root = temp_root("unsafe-root");
    let parent = root.join("backups");
    std::fs::write(&parent, b"foreign").unwrap();
    let child = parent.join("settings");
    let store = SnapshotStore::new(&child, &["settings.toml"], false);

    assert!(store.create_staging().is_err());
    assert_eq!(std::fs::read(&parent).unwrap(), b"foreign");
    let _ = std::fs::remove_dir_all(root);
}
