//! Checks for one-time migration lists.

use super::{BACKUPS_DIR_NAME, CFG_FILES, ROOT_FILES};

/// No migration list may contain the snapshot DIRECTORY.
///
/// `migrate_bundle_data` and `migrate_flat_to_cfg` walk these lists with `fs::copy` and
/// `fs::rename`. They do not recurse, but they do not reject a directory either: on Windows,
/// `fs::rename(data_dir/backups, cfg/backups)` successfully moves the entire tree somewhere
/// `backups_dir()` does not search, making snapshots disappear from the application.
///
/// The plausible breakage is seeing `settings.toml.bak` in `CFG_FILES` and adding `"backups"`
/// beside it. This compiles, and the damage appears only on a machine that already has snapshots.
///
/// The oracle is `BACKUPS_DIR_NAME` itself, which `backups_dir()` uses to build the path, so the
/// test cannot drift from the actual directory name.
#[test]
fn the_migration_lists_never_carry_the_backup_directory() {
    for (label, list) in [("ROOT_FILES", ROOT_FILES), ("CFG_FILES", CFG_FILES)] {
        assert!(
            !list.contains(&BACKUPS_DIR_NAME),
            "{label} names the snapshot directory `{BACKUPS_DIR_NAME}`; the migration would \
             move the whole backup tree out from under backups_dir()"
        );
    }
}
