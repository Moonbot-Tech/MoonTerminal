//! Regression tests for dock-layout schema compatibility and save acknowledgement.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{DockMap, is_compatible_version, save_all_to_path};

/// Sequence making every unwritable test path unique inside one process.
static NEXT_TEMP_PATH: AtomicU64 = AtomicU64::new(1);

/// Return a destination below a path that is deliberately a file, not a directory.
///
/// Returns:
///     Unique destination plus the blocking parent file to remove after the assertion.
fn unwritable_path() -> (PathBuf, PathBuf) {
    let sequence = NEXT_TEMP_PATH.fetch_add(1, Ordering::Relaxed);
    let blocker = std::env::temp_dir().join(format!(
        "moonterminal-blocked-docks-parent-{}-{sequence}",
        std::process::id()
    ));
    std::fs::write(&blocker, b"not a directory").expect("blocking test file must be written");
    (blocker.join("docks.json"), blocker)
}

/// `dock_persist.rs:DOCK_VERSION` changing from 8 back to 7 must fail here; accepting a saved v7
/// layout would leave existing users with Log before News and Core Status.
#[test]
fn version_seven_layouts_are_rebuilt_for_log_last() {
    assert!(!is_compatible_version(Some(7)));
}

/// Changing `dock_persist.rs:save_all_to_path` to return success after an atomic-write error must
/// fail: startup would clear `dock_dirty` and never retry the user's Classic layout.
#[test]
fn dock_save_reports_an_atomic_write_failure() {
    let (path, blocker) = unwritable_path();
    let saved = save_all_to_path(&DockMap::new(), &path);
    std::fs::remove_file(blocker).expect("blocking test file must be removed");
    assert!(!saved);
    assert!(!path.exists());
}
