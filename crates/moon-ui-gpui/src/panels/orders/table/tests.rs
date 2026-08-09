//! Static regressions for delayed Orders-table action authority.

/// `table.rs:flag_toggle_cell` must authorize its captured core inside the Backend update before
/// sending the stop command, and must not publish an optimistic overlay when authority is stale.
///
/// Mutation: insert the overlay before validation or send `set_order_stop` before the guard. Auto
/// could display or submit a stop change for core 7 after the group switched to core 9.
///
/// Returns:
///     Nothing; authority must precede both the command and optimistic UI state.
#[test]
fn stop_toggle_authorizes_before_command_and_optimistic_overlay() {
    let source = include_str!("../table.rs");
    let callback = source
        .split_once("fn flag_toggle_cell(")
        .expect("Orders stop-toggle cell must exist")
        .1;
    let backend_update = callback
        .find("this.backend.update(cx, |b, _|")
        .expect("stop dispatch must use one Backend update");
    let authority = callback
        .find("b.workspace_action_allows_core(Some(&group), core)")
        .expect("stop dispatch must validate current group authority");
    let command = callback
        .find("b.session.set_order_stop(")
        .expect("stop command must remain reachable");
    let overlay = callback
        .find("this.stop_overlay.insert(")
        .expect("authorized stop dispatch must retain its optimistic overlay");

    assert!(backend_update < authority && authority < command && command < overlay);
}

/// Orders must leave Auto selection to the Shell rail and revalidate chart/strategy navigation.
///
/// Mutation: restore the Core-cell Auto writer, call `open_on_main`, or omit the group argument to
/// `open_goto`. A retained row could then select or reveal its old core after a rail switch.
#[test]
fn row_shortcuts_preserve_rail_authority_at_dispatch() {
    let table = include_str!("../table.rs");
    let panel = include_str!("../mod.rs");

    assert!(!panel.contains("select_auto_workspace_core"));
    assert!(table.contains("b.open_on_main_if_authorized("));
    assert!(table.contains("Some(&group)"));
    assert!(table.contains("Some(workspace_group),"));
}
