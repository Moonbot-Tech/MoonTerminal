//! Export-scope identity regressions for the asynchronous Report path picker.

use super::{ReportExportScopeIdentity, report_export_scope_is_current};

/// `actions.rs:report_export_scope_is_current` must reject a group generation or membership drift,
/// while a standalone export remains independent of unrelated workspace generations.
///
/// Mutation: compare only whether either core list is nonempty. A destination chosen for core 7
/// could then export core 9 after Auto navigation.
#[test]
fn stale_group_export_scope_is_rejected_after_path_selection() {
    let requested = ReportExportScopeIdentity {
        workspace_generation: Some(4),
        core_ids: vec![7],
    };

    assert!(report_export_scope_is_current(&requested, &requested));
    assert!(!report_export_scope_is_current(
        &requested,
        &ReportExportScopeIdentity {
            workspace_generation: Some(5),
            core_ids: vec![7],
        }
    ));
    assert!(!report_export_scope_is_current(
        &requested,
        &ReportExportScopeIdentity {
            workspace_generation: Some(4),
            core_ids: vec![9],
        }
    ));

    let standalone = ReportExportScopeIdentity {
        workspace_generation: None,
        core_ids: vec![7],
    };
    assert!(report_export_scope_is_current(&standalone, &standalone));
    assert!(!report_export_scope_is_current(
        &standalone,
        &ReportExportScopeIdentity {
            workspace_generation: None,
            core_ids: vec![9],
        }
    ));
}

/// `actions.rs:ReportPanel::export_report` must rebuild and validate live state after the picker.
///
/// Mutation: move `self.filter(cx)` back before `rx.await` or bypass the identity comparison. The
/// ordered wiring assertion reddens before stale scope can reach `export::run`.
#[test]
fn export_rebuilds_live_filter_after_path_selection() {
    let source = include_str!("../actions.rs");
    let export = source
        .split_once("pub(super) fn export_report(")
        .expect("Report export action must exist")
        .1;
    let awaited = export
        .find("rx.await")
        .expect("path picker must be awaited");
    let live_update = export
        .find("this.update(cx, |this, cx|")
        .expect("panel must be re-read after the picker");
    let identity = export
        .find("report_export_scope_is_current(&requested_scope, &current_scope)")
        .expect("live scope identity must be validated");
    let filter = export
        .find("this.filter(cx)")
        .expect("live filter must be rebuilt");
    let run = export
        .find("export::run(")
        .expect("export must remain reachable");

    assert!(awaited < live_update && live_update < identity && identity < filter && filter < run);
}

/// Removing the live authority pass from `actions.rs:mutate_report_selection` must fail: a menu
/// retained across an Auto rail switch could delete or restore rows on the previous core.
#[test]
fn delete_and_restore_revalidate_every_target_before_dispatch() {
    let source = include_str!("../actions.rs");
    let action = source
        .split_once("pub(super) fn mutate_report_selection(")
        .expect("Report delete/restore action must exist")
        .1;
    let authority = action
        .find("targets.iter().any(|(core_uid, _)|")
        .expect("all captured targets must be checked atomically");
    let guard = action
        .find("workspace_action_allows_core(workspace_group.as_deref(), *core_uid)")
        .expect("group Report must use the live workspace authority");
    let dispatch = action
        .find("set_report_rows_deleted_ids(core_uid, deleted, rec_ids)")
        .expect("delete/restore dispatch must remain present");
    assert!(authority < guard && guard < dispatch);
}
