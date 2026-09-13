//! Regression coverage for delayed Strategies tree modal authority and tree-op dialog width.

use super::{
    clamp_tree_op_dialog_width, folder_delete_authorized, tree_op_authorized,
    tree_op_dialog_client_width, tree_op_field_width,
};

/// Viewport/padding arithmetic for the tree-op card, independent of the named production
/// constants. Restoring `client_w - 2 * 16` inside `clamp_tree_op_dialog_width` makes a 320 px
/// client yield a 288 px card and 256 px fields instead of 320/288. Dropping frame insets in
/// `tree_op_dialog_client_width` keeps a 360 card on a 360 viewport with 20+20 client-frame
/// insets, overflowing MoonUI's 320 px overlay. Changing PAD from 16 to 0 makes
/// `tree_op_field_width(360.0)` return 360 instead of 328 and overflows MoonUI's 16 px pad.
#[test]
fn clamp_tree_op_dialog_width_fits_the_client_viewport_without_subtracting_content_pads() {
    assert_eq!(tree_op_dialog_client_width(320.0, 0.0, 0.0), 320.0);
    assert_eq!(tree_op_dialog_client_width(360.0, 20.0, 20.0), 320.0);
    assert_eq!(clamp_tree_op_dialog_width(360.0, 1920.0), 360.0);
    assert_eq!(clamp_tree_op_dialog_width(360.0, 320.0), 320.0);
    assert_eq!(
        clamp_tree_op_dialog_width(360.0, tree_op_dialog_client_width(360.0, 20.0, 20.0)),
        320.0
    );
    assert_eq!(tree_op_field_width(360.0), 328.0);
    assert_eq!(tree_op_field_width(320.0), 288.0);
    assert_eq!(
        tree_op_field_width(clamp_tree_op_dialog_width(360.0, 320.0)),
        288.0
    );
    assert_eq!(
        tree_op_field_width(clamp_tree_op_dialog_width(
            360.0,
            tree_op_dialog_client_width(360.0, 20.0, 20.0),
        )),
        288.0
    );
}

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

/// `dialogs.rs:delete_folder` must delete the folder's strategies BEFORE the folder command.
///
/// The wire has no "delete a populated folder": `TStratDelete(0, path)` and omission from the
/// folder tree both leave a folder that still holds a strategy, and the core answers with a log
/// line and no event (GateF, 2026-09-13: `Folder delete request ... Demo (not empty or not
/// found)`, three times). Sending the folder alone is a confirmed "Yes" that does nothing.
#[test]
fn folder_delete_empties_the_folder_before_removing_it() {
    let source = include_str!("../dialogs.rs");
    let dispatch = source
        .split_once("fn delete_folder(")
        .expect("folder delete dispatcher must exist")
        .1;
    // The loop must walk the revalidated live snapshot, not the captured one: a row that
    // appeared after confirmation fails the guard, a row that vanished must not be re-sent.
    let strategies = dispatch
        .find("for (id, _) in &current_targets")
        .expect("populated folder must delete every live row under it");
    assert!(dispatch[strategies..].contains("delete_strategy(core, *id)"));
    let folder = dispatch
        .find(".delete_folder(")
        .expect("folder command must follow");
    assert!(strategies < folder);
    let omission = dispatch
        .find(".remove_core_folder(")
        .expect("empty folder on a versioned-tree core still goes by omission");
    assert!(strategies < omission);
}
