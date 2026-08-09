//! Regression tests for Profit Monitor single-flight window intent.

use super::ProfitMonitorOpenRequest;

/// `profit_monitor/window.rs:ProfitMonitorOpenRequest::upgrade` must retain the strongest pending
/// foreground intent. Replacing `|=` with assignment makes the second assertion red and lets a
/// startup restore arriving after a toolbar click downgrade it to a background-only create.
#[test]
fn pending_restore_and_user_open_merge_to_one_foreground_request() {
    let mut restored = ProfitMonitorOpenRequest::new(false);
    restored.upgrade(true);
    assert!(
        restored.activate,
        "the first toolbar click must upgrade restore"
    );

    let mut clicked = ProfitMonitorOpenRequest::new(true);
    clicked.upgrade(false);
    assert!(
        clicked.activate,
        "a later startup restore must not downgrade a user click"
    );
}
