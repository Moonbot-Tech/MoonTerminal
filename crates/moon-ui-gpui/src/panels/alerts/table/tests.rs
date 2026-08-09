//! Wiring checks for delayed Alerts row actions.

/// Replacing any `commit_core` call with the old unconditional commit would let a stale rendered
/// row arm, reassign, or delete a figure after Auto selected another core.
#[test]
fn every_mutating_row_action_uses_current_workspace_authority() {
    let source = include_str!("../table.rs");
    assert_eq!(source.matches(".commit_core(app, core,").count(), 3);
    assert!(!source.contains(".commit(app,"));
}

/// Dropping either settings guard would let an already-open popover remain actionable, or let a
/// delayed gear callback reopen it, after the Alerts group changed its selected Auto core.
#[test]
fn settings_popover_guards_open_state_and_shared_style_writes() {
    let source = include_str!("../table.rs");
    assert!(
        source.contains("workspace_action_allows_core(Some(&toggle_group), toggle_target.core)")
    );
    assert!(source.contains("WorkspaceAuthority::Group(ctx.group.clone())"));
}

/// Alerts coin navigation must revalidate the row's group before raising Main.
///
/// Mutation: call `open_on_main` directly. A retained row callback could then raise a chart for
/// the core that Auto no longer exposes.
#[test]
fn coin_navigation_uses_current_workspace_authority() {
    let source = include_str!("../table.rs");
    assert!(source.contains("b.open_on_main_if_authorized(Some(&group), (core, market), true)"));
}
