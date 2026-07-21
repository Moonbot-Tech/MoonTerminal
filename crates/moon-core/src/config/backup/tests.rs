//! Snapshot behavior on paths where an error can cost user data.

use super::*;

/// Create a unique temporary root for one test without a dev-dependency on `tempfile`.
fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "moonterminal-backup-{}-{tag}-{}",
        std::process::id(),
        STAGING_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp root");
    dir
}

/// Write a file after creating its parent directory.
fn write(path: &Path, body: &str) {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).expect("parent");
    }
    std::fs::write(path, body).expect("write");
}

/// Return the sorted snapshot names in a directory.
fn snapshot_names(backups: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(backups)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| is_snapshot_name(n))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Every snapshot must reread the file rather than reuse previously read contents.
///
/// The plausible mutation this catches is caching source contents, for example by moving the
/// read outside the loop or storing it statically to avoid reading the disk twice. The second
/// snapshot would then preserve the FIRST snapshot's bytes, leaving every backup one save
/// behind and restoring a different state from the one the user expects.
#[test]
fn each_snapshot_rereads_the_file_from_disk() {
    let root = temp_root("bytes");
    let src = root.join("settings.toml");
    let backups = root.join("backups");

    write(&src, "v1");
    let first = snapshot_into(&[&src], &backups, 1_753_100_000_000, 30)
        .expect("first snapshot")
        .expect("a file existed");
    write(&src, "v2");
    let second = snapshot_into(&[&src], &backups, 1_753_100_001_000, 30)
        .expect("second snapshot")
        .expect("a file existed");

    assert_eq!(
        std::fs::read_to_string(first.join("settings.toml")).unwrap(),
        "v1"
    );
    assert_eq!(
        std::fs::read_to_string(second.join("settings.toml")).unwrap(),
        "v2"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Retention prunes by NAME rather than modification time.
///
/// Directories are created in REVERSE chronological order, making mtime order the opposite of
/// name order. An implementation that prunes by mtime would remove the three NEWEST snapshots.
/// This construction also catches any reliance on the enumeration order of `read_dir`.
#[test]
fn retention_keeps_the_newest_by_name_not_by_mtime() {
    let root = temp_root("retention");
    let backups = root.join("backups");
    std::fs::create_dir_all(&backups).unwrap();

    // Real consecutive dates spanning July and early August ensure that names are valid
    // timestamps rather than merely increasing strings.
    let names: Vec<String> = (0..33)
        .map(|i| {
            let (mo, d) = if i < 31 { (7, i + 1) } else { (8, i - 30) };
            format!("2026{mo:02}{d:02}-120000")
        })
        .collect();
    for name in names.iter().rev() {
        let dir = backups.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir.join("settings.toml"), name);
    }

    let removed = prune(&backups, 30, &["settings.toml"]);
    assert_eq!(removed, 3, "33 snapshots minus keep=30");

    let survivors = snapshot_names(&backups);
    assert_eq!(
        survivors,
        names[3..].to_vec(),
        "the three OLDEST must be gone"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Pruning never touches anything the application did not create.
///
/// The plausible mutation is weakening `is_snapshot_name` to "starts with a digit" to support
/// an older naming scheme, which would delete the user's `2026-07-21` directory.
#[test]
fn pruning_never_touches_what_the_app_did_not_write() {
    let root = temp_root("foreign");
    let backups = root.join("backups");
    std::fs::create_dir_all(&backups).unwrap();

    write(&backups.join("notes.txt"), "мои заметки");
    std::fs::create_dir_all(backups.join("2026-07-21")).unwrap();
    std::fs::create_dir_all(backups.join("backup")).unwrap();
    std::fs::create_dir_all(backups.join("20260721-1345")).unwrap();
    let ours = backups.join("20260721-134501");
    std::fs::create_dir_all(&ours).unwrap();
    write(&ours.join("settings.toml"), "x");

    prune(&backups, 0, &["settings.toml"]);

    assert!(!ours.exists(), "our own snapshot must be prunable");
    assert!(backups.join("notes.txt").exists());
    assert!(backups.join("2026-07-21").exists());
    assert!(backups.join("backup").exists());
    assert!(backups.join("20260721-1345").exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// A snapshot containing an unexpected file is not removed wholesale.
///
/// Non-recursive `remove_dir` refuses to remove a non-empty directory, so a file placed there
/// by the user or a cloud client preserves both itself and the snapshot. The plausible mutation
/// is replacing it with `remove_dir_all`.
#[test]
fn a_snapshot_holding_an_unexpected_file_survives_pruning() {
    let root = temp_root("unexpected");
    let backups = root.join("backups");
    let dir = backups.join("20260721-134501");
    std::fs::create_dir_all(&dir).unwrap();
    write(&dir.join("settings.toml"), "x");
    write(&dir.join("заметка.txt"), "не трогать");

    let removed = prune(&backups, 0, &["settings.toml"]);

    assert_eq!(removed, 0, "the directory was not empty, so it must remain");
    assert!(
        dir.join("заметка.txt").exists(),
        "the foreign file must survive"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Two snapshots taken in the same second do not overwrite each other.
///
/// The plausible mutation is removing the collision suffix: `create_dir_all` accepts an
/// existing directory and `fs::copy` overwrites files, so the second save would destroy the
/// snapshot taken by the first. The defect becomes visible only when that snapshot is needed.
#[test]
fn two_snapshots_in_one_second_do_not_overwrite_each_other() {
    let root = temp_root("collision");
    let src = root.join("settings.toml");
    let backups = root.join("backups");
    let ms = 1_753_100_000_000;

    write(&src, "first");
    let a = snapshot_into(&[&src], &backups, ms, 30).unwrap().unwrap();
    write(&src, "second");
    let b = snapshot_into(&[&src], &backups, ms, 30).unwrap().unwrap();

    assert_ne!(a, b, "the second snapshot must take a different directory");
    assert_eq!(
        std::fs::read_to_string(a.join("settings.toml")).unwrap(),
        "first"
    );
    assert_eq!(
        std::fs::read_to_string(b.join("settings.toml")).unwrap(),
        "second"
    );
    assert_eq!(snapshot_names(&backups).len(), 2);
    let _ = std::fs::remove_dir_all(&root);
}

/// A missing config is not a failure and does not create the `backups/` directory.
///
/// The plausible mutation is an unconditional `fs::copy(src, dst)?`. It turns EVERY first
/// launch, and every launch after a corrupt file is moved to `.bak`, into a logged error. A
/// later author may then "fix" it by propagating the error with `?` from saving, causing a
/// failed backup to break the very save it must protect.
#[test]
fn a_missing_config_is_not_a_backup_failure() {
    let root = temp_root("absent");
    let backups = root.join("backups");

    let made = snapshot_into(&[&root.join("settings.toml")], &backups, 0, 30)
        .expect("an absent source is not an error");

    assert!(made.is_none(), "nothing to copy means no snapshot");
    assert!(
        !backups.exists(),
        "backups/ must not be created speculatively"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A source that EXISTS but is not a readable file must fail the entire snapshot rather than
/// disappear from it silently.
///
/// The plausible mutation this catches, and one that existed here, is filtering sources with
/// `p.is_file()`. It returns `false` both when a file is absent and when metadata cannot be
/// read, so an unreadable `servers.enc` silently disappeared while a ONE-file snapshot was
/// published as successful. Saving then overwrote both files, leaving the rollback copy without
/// API keys precisely when it was needed.
///
/// A directory in place of a file is a portable way to produce "exists, but is not a file."
#[test]
fn an_unreadable_source_fails_instead_of_publishing_a_partial_snapshot() {
    let root = temp_root("partial");
    let good = root.join("settings.toml");
    let backups = root.join("backups");
    write(&good, "v1");
    let bogus = root.join("servers.enc");
    std::fs::create_dir_all(&bogus).unwrap();

    let res = snapshot_into(&[&good, &bogus], &backups, 0, 30);

    assert!(
        res.is_err(),
        "a source that exists but is not a file must fail the snapshot"
    );
    assert!(
        snapshot_names(&backups).is_empty(),
        "nothing may be published when a source could not be read"
    );
    let leftovers: Vec<String> = std::fs::read_dir(&backups)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with(STAGING_PREFIX))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        leftovers.is_empty(),
        "staging dirs must never survive: {leftovers:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
