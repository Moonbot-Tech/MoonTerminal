//! Static action-authority regressions for group-owned chart panels.

/// Removing a dispatch-time `workspace_action_allows_core` guard from `trade.rs`, `render.rs`, or
/// `render_input.rs` must fail: a stale chart could trade or navigate a core other than the Auto
/// rail selection.
#[test]
fn every_chart_command_and_navigation_path_revalidates_auto_authority() {
    let trade = include_str!("trade.rs");
    for command in [
        "manual chart order blocked",
        "chart start-cross cancel",
        "hotkey cancel hovered order failed",
        "hotkey split hovered order failed",
        "manual chart move line:",
    ] {
        let command_at = trade
            .find(command)
            .unwrap_or_else(|| panic!("missing chart action marker: {command}"));
        let prefix = &trade[..command_at];
        assert!(
            prefix
                .rfind("workspace_action_allows_core")
                .is_some_and(|guard_at| command_at - guard_at < 2_500),
            "{command} must follow a nearby live workspace-authority guard"
        );
    }
    let menu = trade
        .split("pub(super) fn try_open_order_menu(")
        .nth(1)
        .and_then(|tail| tail.split("pub fn cancel_hovered_order(").next())
        .expect("chart order-menu producer must remain present");
    assert!(
        menu.find("workspace_action_allowed").unwrap()
            < menu
                .find("workspace_group: self.workspace_group.clone()")
                .unwrap(),
        "the chart menu must validate before carrying group authority into delayed callbacks"
    );

    let render = include_str!("render.rs");
    let action_callback = render
        .split("fn action_button(")
        .nth(1)
        .and_then(|tail| tail.split("impl Render for ChartPanel").next())
        .expect("chart action-button callback must remain present");
    assert!(
        action_callback.contains("workspace_action_allows_core(workspace_group.as_deref(), core)")
            && action_callback
                .find("workspace_action_allows_core")
                .unwrap()
                < action_callback.find("cancel_market_buys").unwrap()
            && action_callback
                .find("workspace_action_allows_core")
                .unwrap()
                < action_callback.find("toggle_panic_sell").unwrap(),
        "Cancel Buy and Panic Sell must revalidate before dispatch"
    );

    let input = include_str!("render_input.rs");
    let navigation = input
        .split("if let Some((core, market)) = this.input.pending_to_main.take()")
        .nth(1)
        .and_then(|tail| tail.split("if input_changed || opened_to_main").next())
        .expect("AddToChart-to-Main navigation callback must remain present");
    assert!(
        navigation.find("workspace_action_allows_core").unwrap()
            < navigation.find("open_on_main").unwrap(),
        "old chart navigation must not bypass the Auto rail"
    );
}

/// Removing the group argument from `main_stack.rs` or `add_stack.rs` must fail: chart callbacks
/// would become explicitly unscoped and the live guards above would always allow stale cores.
#[test]
fn chart_stacks_pass_their_workspace_group_into_every_panel() {
    let main = include_str!("../../chart_tabs/main_stack.rs");
    let add = include_str!("../../chart_tabs/add_stack.rs");
    assert!(main.contains(
        "ChartPanel::new_main(\n                backend,\n                Some(workspace_group),"
    ));
    assert!(add.contains(
        "ChartPanel::new_addto(backend, workspace_group, num, bucket, epoch, theme, cx)"
    ));
}
