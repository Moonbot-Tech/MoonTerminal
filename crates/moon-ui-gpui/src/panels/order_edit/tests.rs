//! Dispatch-authority regressions for the active-order editor.

use super::submit::order_edit_core_is_authorized;

/// `submit.rs:order_edit_core_is_authorized` must require the captured core only while Auto owns
/// the originating group; Classic and global/chart authority keep their prior behavior.
///
/// Mutation: accept any nonempty Auto scope. An editor opened on core 7 could then submit after the
/// workspace moves to core 9.
#[test]
fn auto_order_editor_requires_its_origin_core_to_remain_effective() {
    assert!(order_edit_core_is_authorized(true, &[7], 7));
    assert!(!order_edit_core_is_authorized(true, &[9], 7));
    assert!(!order_edit_core_is_authorized(true, &[], 7));
    assert!(order_edit_core_is_authorized(false, &[9], 7));
    assert!(order_edit_core_is_authorized(false, &[], 7));
}

/// `submit.rs:apply` must validate Auto authority inside the Backend update before its first command.
///
/// Mutation: move validation between `move_order` and `update_order_stops`. A changed price would
/// be queued before stale scope rejects the stop form, creating a partial user action.
#[test]
fn order_editor_revalidates_before_the_first_command() {
    let source = include_str!("submit.rs");
    let apply = source
        .split_once("pub(super) fn apply(")
        .expect("order-edit submit entry point must exist")
        .1;
    let backend_update = apply
        .find("backend.update(cx")
        .expect("commands must share one Backend update");
    let validation = apply
        .find("order_edit_core_is_authorized(")
        .expect("Auto membership must be revalidated");
    let move_order = apply
        .find("b.session.move_order(")
        .expect("price move command must remain reachable");
    let update_stops = apply
        .find("b.session.update_order_stops(")
        .expect("stop update command must remain reachable");

    assert!(backend_update < validation && validation < move_order && validation < update_stops);
}

/// Every order-editor constructor must retain its originating group authority.
///
/// Mutation: pass `None` from Orders or ChartLine. The wiring check reddens before Auto can let a
/// stale order edit escape the rail-selected core.
///
/// Returns:
///     Nothing; group and chart callers must retain their distinct authority wiring.
#[test]
fn order_editor_callers_preserve_group_and_chart_authority() {
    let compact =
        |source: &str| -> String { source.chars().filter(|ch| !ch.is_whitespace()).collect() };
    let orders = compact(include_str!("../orders/table.rs"));
    let menu = compact(include_str!("../../controls/coin_menu.rs"));
    let chart = compact(include_str!("../chart/trade.rs"));

    assert!(orders.contains("workspace_group:Some(workspace_group)"));
    assert!(orders.contains("open_order_edit(backend,Some(group),core,uid,window,app)"));
    assert!(menu.contains(
        "open_order_edit(backend_e.clone(),workspace_group.clone(),core,uid,window,app,)"
    ));
    assert!(menu.contains(
        "backend_e.update(app,|b,_|{workspace_action_allows_cores(b,workspace_group.as_deref(),&[core])})"
    ));
    assert!(chart.contains("workspace_group:self.workspace_group.clone()"));
}
