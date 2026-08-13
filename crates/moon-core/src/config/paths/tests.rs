//! Checks for one-time migration lists.

use super::{
    BACKUPS_DIR_NAME, CFG_FILES, DAMAGED_REPORTS_DIR_NAME, ROOT_FILES, backups_dir,
    settings_backups_dir, strategies_backups_dir, update_root_for_executable,
    validate_update_nonce,
};
use std::path::Path;

/// No migration list may contain a protected snapshot DIRECTORY.
///
/// `migrate_bundle_data` and `migrate_flat_to_cfg` walk these lists with `fs::copy` and
/// `fs::rename`. They do not recurse, but they do not reject a directory either: on Windows,
/// moving `backups/` or `damaged-reports/` succeeds and makes preserved data disappear from its
/// canonical lookup path.
///
/// The plausible breakage is adding either directory name to a migration list beside a similarly
/// named flat file. This compiles, and the damage appears only on a machine with preserved data.
///
/// The oracle uses the same constants that build both canonical paths, so it cannot drift from the
/// actual directory names.
#[test]
fn the_migration_lists_never_carry_the_backup_directory() {
    for directory in [BACKUPS_DIR_NAME, DAMAGED_REPORTS_DIR_NAME] {
        for (label, list) in [("ROOT_FILES", ROOT_FILES), ("CFG_FILES", CFG_FILES)] {
            assert!(
                !list.contains(&directory),
                "{label} names the protected directory `{directory}`; the migration would move \
                 the whole tree away from its canonical path"
            );
        }
    }
}

/// Removing either `.join(...)` in `paths.rs` would mix settings and strategy snapshots in the
/// shared parent, letting one subsystem's retention inspect the other subsystem's files.
#[test]
fn backup_subsystems_have_distinct_children_of_the_canonical_root() {
    let root = backups_dir();
    let settings = settings_backups_dir();
    let strategies = strategies_backups_dir();

    assert_eq!(settings.parent(), Some(root.as_path()));
    assert_eq!(strategies.parent(), Some(root.as_path()));
    assert_ne!(settings, strategies);
}

/// Moving the update root into `cfg/`, `data/`, `logs/`, or `backups/` would let cleanup or a
/// failed replacement mutate portable user data. The explicit executable fixture is independent
/// of the process running this test.
#[test]
fn update_artifacts_have_a_dedicated_child_beside_the_executable() {
    let executable = Path::new("C:/portable/MoonTerminal.exe");
    let root = update_root_for_executable(executable);

    assert_eq!(root, Path::new("C:/portable/.moonterminal-update"));
    let name = root.file_name().and_then(|value| value.to_str());
    assert!(!["cfg", "data", "logs", "backups"].contains(&name.unwrap_or_default()));
}

/// Accepting traversal, separators, or absolute syntax in a transaction identifier would let the
/// staged executable escape the dedicated update root and overlap portable user data.
#[test]
fn update_nonce_accepts_only_fixed_lowercase_hex() {
    assert!(validate_update_nonce("0123456789abcdef0123456789abcdef").is_ok());
    for rejected in [
        "../cfg",
        "0123456789abcdef/123456789abcdef",
        "C:\\portable\\cfg\\settings.toml",
        "0123456789ABCDEF0123456789ABCDEF",
        "abc",
    ] {
        assert!(
            validate_update_nonce(rejected).is_err(),
            "accepted {rejected}"
        );
    }
}
