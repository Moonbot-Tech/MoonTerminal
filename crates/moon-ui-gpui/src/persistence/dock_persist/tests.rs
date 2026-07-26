//! Regression tests for dock-layout schema compatibility.

use super::is_compatible_version;

/// `dock_persist.rs:DOCK_VERSION` changing from 8 back to 7 must fail here; accepting a saved v7
/// layout would leave existing users with Log before News and Core Status.
#[test]
fn version_seven_layouts_are_rebuilt_for_log_last() {
    assert!(!is_compatible_version(Some(7)));
}
