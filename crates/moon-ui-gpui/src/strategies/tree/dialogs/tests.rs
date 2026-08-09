//! Regression coverage for delayed Strategies tree modal authority.

use super::{folder_delete_authorized, tree_op_authorized};

/// `dialogs.rs:folder_delete_authorized` must compare generation, the complete child snapshot,
/// and disabled state; removing any term can delete a folder changed behind its confirmation.
#[test]
fn stale_folder_confirmation_refuses_child_or_state_drift() {
    let captured = vec![(10, false), (20, false)];

    assert!(folder_delete_authorized(
        Some(7),
        Some(7),
        Some(&[11]),
        11,
        &captured,
        &captured,
    ));
    assert!(!folder_delete_authorized(
        Some(7),
        Some(7),
        Some(&[11]),
        11,
        &captured,
        &[(10, false), (20, false), (30, false)],
    ));
    assert!(!folder_delete_authorized(
        Some(7),
        Some(7),
        Some(&[11]),
        11,
        &captured,
        &[(10, false), (20, true)],
    ));
}

/// `dialogs.rs:tree_op_authorized` must retain the Auto generation check; removing it lets an old
/// create or rename modal dispatch after an A -> B -> A rail round trip.
#[test]
fn create_and_rename_modals_expire_on_workspace_generation_change() {
    assert!(tree_op_authorized(Some(7), Some(7), Some(&[11]), 11));
    assert!(!tree_op_authorized(Some(7), Some(9), Some(&[11]), 11));
    assert!(tree_op_authorized(None, None, None, 11));
}

/// `dialogs.rs:request_delete_folder` and `delete_folder` must wire the exact captured snapshot to
/// the guard; bypassing either side would leave the pure authority rule unused in production.
#[test]
fn folder_delete_dispatch_uses_the_captured_snapshot() {
    let source = include_str!("../dialogs.rs");
    assert!(source.contains("targets: folder_targets(&under)"));
    assert!(source.contains("workspace_generation: self.action_workspace_generation(cx)"));
    let dispatch = source
        .split_once("fn delete_folder(")
        .expect("folder delete dispatcher must exist")
        .1;
    assert!(dispatch.contains("folder_delete_authorized("));
}
