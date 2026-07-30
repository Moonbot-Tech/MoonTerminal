//! Checks for one-time migration lists.

use super::{BACKUPS_DIR_NAME, CFG_FILES, DAMAGED_REPORTS_DIR_NAME, ROOT_FILES};

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
