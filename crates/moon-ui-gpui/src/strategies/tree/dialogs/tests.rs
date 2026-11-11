//! Regression coverage for delayed Strategies tree modal authority and tree-op dialog width.

use super::{
    EmptyFolderCleanupRequest, TreeOp, clamp_tree_op_dialog_width, confirmed_empty_folder_plan,
    empty_folder_cleanup_request, folder_delete_authorized, op_uses_input, tree_op_authorized,
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

/// Enter's `ConfirmDialog` action is captured before MoonUI's default close and shares OK's helper.
#[test]
fn enter_and_ok_share_tree_dialog_confirmation() {
    let source = include_str!("../dialogs.rs");
    let open = source
        .split_once("fn open_op_dialog(")
        .expect("tree dialog opener must exist")
        .1
        .split_once("fn request_delete_target(")
        .expect("tree dialog opener end must be found")
        .0;
    assert!(open.contains("window.open_unique_moon_dialog("));
    assert!(open.contains(".capture_action("));
    assert!(open.contains("gpui_component::dialog::ConfirmDialog"));
    assert!(open.contains("this.confirm_op_dialog_and_report(window, cx)"));
    assert!(open.contains("window.focus(&dialog_focus, cx)"));
    let ensure = source
        .split_once("fn ensure_op_input(")
        .expect("tree dialog input setup must exist")
        .1
        .split_once("fn close_op_dialog(")
        .expect("tree dialog input setup end must be found")
        .0;
    assert!(ensure.contains("self.op.as_ref().is_some_and(op_uses_input)"));

    let footer = source
        .split_once("fn op_dialog_footer(")
        .expect("tree dialog footer must exist")
        .1
        .split_once("impl StrategiesView")
        .expect("tree dialog footer end must be found")
        .0;
    assert!(footer.contains("this.confirm_op_dialog(window, cx)"));
    assert!(footer.contains("dialogs.name_required"));
    let report = source
        .split_once("fn confirm_op_dialog_and_report(")
        .expect("shared confirmation reporter must exist")
        .1
        .split_once("fn confirm_op_dialog(")
        .expect("shared confirmation reporter end must be found")
        .0;
    assert!(report.contains("self.confirm_op_dialog(window, cx)"));
    assert!(report.contains("dialogs.name_required"));
}

/// Creating an unused input for a confirmation leaves Enter on MoonUI's default close action rather
/// than the content's capture handler, dismissing the dialog without dispatching the operation.
#[test]
fn confirmation_ops_do_not_create_input_state() {
    let confirmation = TreeOp::ConfirmDeleteEmptyFolders {
        candidates: vec![(7, "empty".to_string())],
        workspace_generation: Some(9),
    };
    assert!(!op_uses_input(&confirmation));

    let create = TreeOp::CreateFolder {
        core: 7,
        target: String::new(),
        workspace_generation: Some(9),
    };
    assert!(op_uses_input(&create));
}

/// Restoring a direct dispatcher call in either bulk request would delete before the user confirms.
/// The staging helper may dispatch only an empty plan to preserve the existing informational note.
#[test]
fn bulk_folder_delete_requests_stage_confirmation_without_queueing() {
    let source = include_str!("../dialogs.rs");
    for (entry, end) in [
        (
            "fn request_delete_target(",
            "fn request_delete_all_empty_folders(",
        ),
        (
            "fn request_delete_all_empty_folders(",
            "fn request_empty_folder_cleanup(",
        ),
    ] {
        let body = source
            .split_once(entry)
            .expect(entry)
            .1
            .split_once(end)
            .expect(end)
            .0;
        assert!(body.contains("self.request_empty_folder_cleanup(plan, window, cx)"));
        assert!(!body.contains("delete_empty_folders("));
        assert!(!body.contains(".delete_empty_folder("));
    }

    let staging = source
        .split_once("fn request_empty_folder_cleanup(")
        .expect("bulk confirmation staging must exist")
        .1
        .split_once("pub(super) fn all_empty_folder_candidates(")
        .expect("bulk confirmation staging end must be found")
        .0;
    assert!(staging.contains("empty_folder_cleanup_request("));
    assert!(staging.contains("EmptyFolderCleanupRequest::Confirm(op)"));
    assert!(staging.contains("self.op = Some(op)"));
    assert!(staging.contains("self.open_op_dialog(window, cx);"));
    assert!(!staging.contains(".delete_empty_folder("));

    let rows = Vec::new();
    let candidates = vec![(7, "empty-a".to_string()), (8, "empty-b".to_string())];
    let plan = super::ops::empty_folder_delete_plan(
        &[(7, rows.as_slice()), (8, rows.as_slice())],
        &candidates,
    );
    let EmptyFolderCleanupRequest::Confirm(TreeOp::ConfirmDeleteEmptyFolders {
        candidates: staged,
        workspace_generation: Some(9),
    }) = empty_folder_cleanup_request(plan, Some(9))
    else {
        panic!("a non-empty plan must stage the bulk confirmation");
    };
    assert_eq!(staged, candidates);

    let empty = empty_folder_cleanup_request(Default::default(), None);
    assert!(matches!(
        empty,
        EmptyFolderCleanupRequest::Report(plan) if plan.targets.is_empty()
    ));
}

/// Both bulk entry points must retain the conditional dispatcher, which never deletes a strategy
/// and reports exactly how many occupied folders the confirmation-time planner skipped.
#[test]
fn bulk_folder_delete_is_conditional_and_never_deletes_strategies() {
    let source = include_str!("../dialogs.rs");
    let dispatch = source
        .split_once("fn delete_empty_folders(")
        .expect("bulk empty-folder dispatcher must exist")
        .1
        .split_once("pub(super) fn request_delete_selection(")
        .expect("bulk dispatcher end must be found")
        .0;
    assert!(dispatch.contains(".delete_empty_folder("));
    assert!(!dispatch.contains(".delete_strategy("));
    assert!(dispatch.contains("skipped: plan.skipped_non_empty"));
}

/// Reusing the request-time plan would delete a folder that gained a strategy behind the dialog.
/// Confirmation must revalidate scope and classify the captured identities against current rows.
#[test]
fn bulk_folder_delete_confirmation_replans_current_snapshot() {
    let source = include_str!("../dialogs.rs");
    let dialog = source
        .split_once("fn confirm_op_dialog(")
        .expect("shared operation confirmation must exist")
        .1
        .split_once("pub(in crate::strategies) fn open_op_dialog(")
        .expect("shared operation confirmation end must be found")
        .0;
    assert!(dialog.contains("self.confirm_delete_empty_folders("));
    let confirm = source
        .split_once("fn confirm_delete_empty_folders(")
        .expect("bulk empty-folder confirmation must exist")
        .1
        .split_once("fn delete_empty_folders(")
        .expect("bulk confirmation end must be found")
        .0;
    let replan = confirm
        .find("confirmed_empty_folder_plan(")
        .expect("confirmation must reclassify the captured candidates");
    let dispatch = confirm
        .find("self.delete_empty_folders(plan, window, cx);")
        .expect("only the fresh plan may reach deletion");
    assert!(replan < dispatch);

    let candidates = vec![(7, "empty".to_string()), (7, "occupied-later".to_string())];
    let before = Vec::new();
    let opened = confirmed_empty_folder_plan(
        &candidates,
        Some(7),
        Some(7),
        Some(&[7]),
        &[(7, before.as_slice())],
    )
    .expect("matching workspace authority must permit re-planning");
    assert_eq!(opened.targets.len(), 2);

    let current = [moon_core::feed::StrategyRow {
        id: 42,
        name: "arrived".to_string(),
        kind: "Long".to_string(),
        kind_ordinal: 1,
        folder_path: "occupied-later".to_string(),
        checked: false,
        is_short: false,
        fields: Vec::new(),
    }];
    let confirmed = confirmed_empty_folder_plan(
        &candidates,
        Some(7),
        Some(7),
        Some(&[7]),
        &[(7, current.as_slice())],
    )
    .expect("an occupied candidate must be skipped, not cancel its empty sibling");
    assert_eq!(confirmed.skipped_non_empty, 1);
    assert_eq!(
        confirmed
            .targets
            .iter()
            .map(|target| target.path.as_str())
            .collect::<Vec<_>>(),
        vec!["empty"]
    );
    assert_eq!(
        confirmed.targets[0].expected_placements,
        vec![(42, "occupied-later".to_string())]
    );
    assert!(
        confirmed_empty_folder_plan(
            &candidates,
            Some(7),
            Some(9),
            Some(&[7]),
            &[(7, current.as_slice())],
        )
        .is_none(),
        "a generation change must cancel the whole batch before any queue call"
    );
}

/// The confirmation names both the folder and core counts and warns that occupied folders remain.
#[test]
fn bulk_folder_delete_confirmation_text_has_counts_and_safety_note() {
    let _locale = crate::test_locale::force("en");
    let text = rust_i18n::t!("strat.delete_empty_folders_confirm", n = 3, cores = 2);
    assert_eq!(
        text,
        "Delete 3 empty folders on 2 cores? Folders containing strategies are left untouched."
    );
}

/// The occupied-folder count is user-visible and explicitly says that containing strategies is why
/// those folders stayed in place.
#[test]
fn bulk_folder_delete_note_reports_occupied_skips() {
    let _locale = crate::test_locale::force("en");
    let note = rust_i18n::t!("strat.note_empty_folders_skipped", deleted = 2, skipped = 3);
    assert!(note.contains('2') && note.contains('3'));
    assert!(note.contains("because they contain strategies"));
}

/// Dropping the hidden-kind filter from `create_kind_list` puts ChartWall back in the create menu;
/// dropping the existing MoonShot reorder would separately regress the menu's primary choice.
#[test]
fn create_kind_list_hides_chart_wall_and_keeps_moonshot_first() {
    let kind = |ordinal, name: &str| moon_core::feed::SchemaKind {
        ordinal,
        name: name.to_string(),
        sections: Vec::new(),
    };
    let schema = moon_core::feed::StrategySchemaModel {
        kinds: vec![
            kind(2, "Drops"),
            kind(19, "Unknown"),
            kind(20, "MoonHook"),
            kind(6, "MoonShot"),
        ],
    };

    assert_eq!(
        super::create_kind_list(&schema),
        vec![
            (6, "MoonShot".to_string()),
            (2, "Drops".to_string()),
            (20, "MoonHook".to_string()),
        ]
    );
}
