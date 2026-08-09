//! Regression tests for Auto-workspace Shell presentation policy.

use crate::workspace::{WorkspaceCoreStatus, WorkspaceRailDensity};

use moon_ui::DockTopologyNode;

use super::{
    auto_only_detached_panel_names, default_auto_workspace_topology, fitted_auto_rail_width,
    workspace_status_label_visible,
};
use crate::window::detached::DetachedSpec;

/// Catches making Ready rows render the same trailing label as failures in
/// `shell/workspace.rs:workspace_status_label_visible`; long server names would be truncated by a
/// redundant "Ready" even though the green dot already communicates health.
#[test]
fn ready_rows_keep_name_width_while_failures_remain_explicit() {
    assert!(!workspace_status_label_visible(
        WorkspaceCoreStatus::Ready,
        WorkspaceRailDensity::Full,
    ));
    for status in [
        WorkspaceCoreStatus::Disabled,
        WorkspaceCoreStatus::Unavailable,
        WorkspaceCoreStatus::Problem,
    ] {
        assert!(workspace_status_label_visible(
            status,
            WorkspaceRailDensity::Full,
        ));
        assert!(!workspace_status_label_visible(
            status,
            WorkspaceRailDensity::Compact,
        ));
    }
}

/// Replacing `shell/workspace.rs:fitted_auto_rail_width` with the raw preference must fail: a
/// narrow group window would squeeze out the dock instead of locally fitting the shared rail.
#[test]
fn global_rail_preference_is_fitted_per_window_without_redefining_it() {
    let preferred = 500.0;
    assert_eq!(fitted_auto_rail_width(preferred, 1_000.0, 1.0), 500.0);
    assert_eq!(
        fitted_auto_rail_width(preferred, 700.0, 1.0),
        280.0,
        "the local window may fit the rail while the caller retains the 500 px preference"
    );

    let init = include_str!("../init.rs");
    let bounds_observer = init
        .split("cx.observe_window_bounds(window")
        .nth(1)
        .and_then(|tail| tail.split(".detach();").next())
        .expect("Shell must keep its native bounds observer");
    assert!(
        bounds_observer.contains("this.sync_auto_rail_width(window, cx)"),
        "native resize and DPI changes must reconcile the live MoonResizableState"
    );
}

/// Removing Log from the upper tabs or restoring the third compact split in
/// `shell/workspace.rs:default_auto_workspace_topology` must fail: first-time Auto users would
/// again get an independently split Log instead of the requested shared top tab strip.
#[test]
fn first_auto_workspace_keeps_log_in_the_flexible_upper_tabs() {
    let topology = default_auto_workspace_topology();
    let DockTopologyNode::Split {
        horizontal,
        items,
        sizes,
    } = topology.center
    else {
        panic!("the first Auto topology must be a vertical split");
    };
    assert!(!horizontal);
    assert_eq!(items.len(), 2);
    assert_eq!(sizes.len(), 2);
    assert_eq!(sizes[0], None, "the upper operational tabs must flex");
    let DockTopologyNode::Tabs { names } = &items[0] else {
        panic!("the flexible upper region must retain the primary tab strip");
    };
    assert_eq!(names.first().map(String::as_str), Some("ChartTabs"));
    assert!(names.iter().any(|name| name == "Report"));
    assert!(names.iter().any(|name| name == "Log"));
    assert!(!names.iter().any(|name| name == "Orders"));
    assert_eq!(
        items[1],
        DockTopologyNode::Panel {
            name: "Orders".to_string()
        }
    );
}

/// Replacing the Orders preferred height in
/// `shell/workspace.rs:default_auto_workspace_topology` with the old 260 px value must fail:
/// first-time Auto users would lose the four additional visible order rows they requested.
#[test]
fn first_auto_workspace_adds_exactly_four_rows_to_orders() {
    let topology = default_auto_workspace_topology();
    let DockTopologyNode::Split { sizes, .. } = topology.center else {
        panic!("the first Auto topology must be a vertical split");
    };
    assert_eq!(
        sizes[1],
        Some(260.0 + 4.0 * crate::design::TABLE_ROW_H),
        "Orders must be exactly four design table rows taller than the old seed"
    );
}

/// Reordering the saved-topology lookup after the fallback or removing the default-only Report
/// activation in `Shell::apply_workspace_mode` must fail: a valid user layout would be reset, or a
/// first-time Auto session would open on whichever tab happened to be active before the switch.
#[test]
fn saved_auto_topology_wins_while_only_the_fallback_activates_report() {
    let source = include_str!("../workspace.rs");
    let auto_branch = source
        .split("WorkspaceMode::AutoTrading =>")
        .nth(1)
        .and_then(|tail| tail.split("WorkspaceMode::Classic =>").next())
        .expect("workspace mode application must retain distinct Auto and Classic branches");
    let saved = auto_branch
        .find(".auto_dock_topology()")
        .expect("persisted Auto topology must be consulted");
    let fallback = auto_branch
        .find(".unwrap_or_else(default_auto_workspace_topology)")
        .expect("only a missing topology may use the first-run seed");
    let default_guard = auto_branch
        .find("if is_default_topology")
        .expect("Report activation must remain limited to the fallback topology");
    let report = auto_branch
        .find("dock.activate_panel_by_name(\"Report\"")
        .expect("the first-run layout must activate Report");
    assert!(saved < fallback && fallback < default_guard && default_guard < report);
}

/// Removing the Classic-name exclusion in `auto_only_detached_panel_names` must fail: independently
/// debounced stale dock and detached records would create two Auto panel identities with one name.
#[test]
fn live_classic_panel_name_outranks_a_stale_detached_record() {
    let classic = vec!["ChartTabs".to_string(), "Orders".to_string()];
    let detached = vec![
        DetachedSpec::new("G1".to_string(), "Orders".to_string()),
        DetachedSpec::new("G1".to_string(), "Log".to_string()),
        DetachedSpec::new("G1".to_string(), "Log".to_string()),
        DetachedSpec::new("G2".to_string(), "Assets".to_string()),
    ];

    assert_eq!(
        auto_only_detached_panel_names("G1", &classic, &detached),
        vec!["Log".to_string()],
        "only a unique detached name absent from the live Classic dock needs an Auto instance"
    );
}

/// Replacing `resize_panel_silently` in `shell/workspace.rs:sync_auto_rail_width` with the emitting
/// resize API must fail: a narrow window would publish its fitted width as a new global preference
/// and shrink every other Auto workspace.
#[test]
fn window_local_rail_fitting_does_not_emit_a_preference_resize() {
    let source = include_str!("../workspace.rs");
    let body = source
        .split("pub(super) fn sync_auto_rail_width")
        .nth(1)
        .and_then(|tail| tail.split("pub(super) fn apply_workspace_mode").next())
        .expect("Auto rail synchronization must retain a bounded implementation");
    assert!(body.contains("resize_panel_silently"));
    assert!(!body.contains("state.resize_panel("));
}
