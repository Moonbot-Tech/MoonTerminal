//! Regression tests for Auto-workspace Shell presentation policy.

#[cfg(feature = "debug-tools")]
use gpui::{AppContext as _, Context, EventEmitter};

use crate::workspace::{WorkspaceCoreStatus, WorkspaceRailDensity, WorkspaceRosterRow};

use moon_core::config::AUTO_WORKSPACE_RAIL_WIDTH_MIN;
use moon_core::feed::{ConnStatus, CoreInitStep, CoreStartupState, CoreStartupStatus};
#[cfg(feature = "debug-tools")]
use moon_ui::DockEvent;
use moon_ui::{DockTopologyByName, DockTopologyNode};

#[cfg(feature = "debug-tools")]
use super::{
    DeferredAutoTopologyGuard, auto_workspace_tab_to_persist,
    auto_workspace_topology_is_persistable, defer_auto_topology_guard_release,
};
use super::{
    RailItem, append_core_section_items, auto_classic_only_panel_names,
    auto_only_detached_panel_names, auto_workspace_activation_fallback,
    auto_workspace_tab_is_eligible, core_rail_metrics, default_auto_workspace_topology,
    ensure_auto_topology_contains_panel, fitted_auto_rail_width, icon_workspace_summary,
    resolved_auto_workspace_tab, workspace_core_tooltip, workspace_status_label_visible,
};
use crate::window::detached::DetachedSpec;

/// Minimal event source used to exercise GPUI's queued subscription delivery.
#[cfg(feature = "debug-tools")]
struct DockEventSource;

#[cfg(feature = "debug-tools")]
impl EventEmitter<DockEvent> for DockEventSource {}

/// Minimal owner with the same deferred topology-guard contract as Shell.
#[cfg(feature = "debug-tools")]
struct AutoGuardHarness {
    applying_topology: bool,
    guard_generation: u64,
    saved_tab: String,
    topology_writes: usize,
}

#[cfg(feature = "debug-tools")]
impl DeferredAutoTopologyGuard for AutoGuardHarness {
    fn release_auto_topology_guard(&mut self, generation: u64) {
        if self.guard_generation == generation {
            self.applying_topology = false;
        }
    }
}

/// Build one rail row without coupling branch-shape expectations to roster derivation.
fn rail_row(core: u64) -> WorkspaceRosterRow {
    WorkspaceRosterRow {
        fault: None,
        core,
        name: format!("Core {core}"),
        group: "G1".to_string(),
        status: WorkspaceCoreStatus::Ready,
        selectable: true,
        selected: false,
        connection: Some(ConnStatus::Ready),
        startup: CoreStartupStatus::default(),
    }
}

/// Replacing the problem hover with the generic core identity line must fail: the operator would
/// again lose the exact live stage and the transport evidence needed to explain a stuck server.
#[test]
fn problem_hover_keeps_connection_stage_and_complete_startup_diagnostics() {
    let mut row = rail_row(18);
    row.status = WorkspaceCoreStatus::Problem;
    row.connection = Some(ConnStatus::Failed("authentication refused".to_string()));
    row.startup = CoreStartupStatus {
        state: CoreStartupState::Failed,
        current_step: Some(CoreInitStep::AuthCheck),
        completed_mask: 1,
        path_mtu_bytes: Some(1400),
        ..CoreStartupStatus::default()
    };

    let tooltip = workspace_core_tooltip(&row);

    assert!(tooltip.contains("authentication refused"));
    assert!(tooltip.contains("(1/8)"));
    assert!(tooltip.contains("MTU: 1400"));
}

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

/// Replacing `shell/workspace.rs:fitted_auto_rail_width` with the raw preference or flooring its
/// maximum at Compact must fail: a narrow group window would squeeze the dock or choose the wrong
/// density instead of fitting the shared rail down to Icon.
#[test]
fn global_rail_preference_is_fitted_per_window_without_redefining_it() {
    let preferred = 500.0;
    assert_eq!(fitted_auto_rail_width(preferred, 1_000.0, 1.0), 500.0);
    assert_eq!(
        fitted_auto_rail_width(preferred, 700.0, 1.0),
        280.0,
        "the local window may fit the rail while the caller retains the 500 px preference"
    );
    let narrow_width = fitted_auto_rail_width(preferred, 520.0, 1.0);
    assert_eq!(narrow_width, 100.0);
    assert_eq!(520.0 - narrow_width, 420.0);
    assert_eq!(
        crate::workspace::workspace_rail_density(narrow_width),
        WorkspaceRailDensity::Icon
    );

    let workspace = include_str!("../workspace.rs");
    let body = workspace
        .split("pub(super) fn workspace_body")
        .nth(1)
        .and_then(|tail| tail.split("fn workspace_rail").next())
        .expect("workspace body must keep a bounded implementation");
    let fitted = body
        .find("let rail_width = fitted_auto_rail_width(")
        .expect("body must derive its effective fitted rail width");
    let density = body
        .find("workspace_rail_density(rail_width)")
        .expect("density must use the fitted effective width");
    assert!(fitted < density);

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
    assert!(!names.iter().any(|name| name == "News"));
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

/// Removing the topology insertion in `shell/workspace.rs:ensure_auto_topology_contains_panel`
/// must fail: an Auto reveal for Assets omitted by saved `auto_dock.json` would remain a silent
/// no-op rather than making Assets available for activation.
#[test]
fn auto_surface_reveal_inserts_absent_panel_into_saved_topology() {
    let mut topology = DockTopologyByName::tab_preset(["ChartTabs", "Report"]);

    ensure_auto_topology_contains_panel(&mut topology, "Assets");

    assert!(
        topology.panel_names().iter().any(|name| name == "Assets"),
        "the requested Auto surface must become part of the saved topology"
    );
}

/// Returning the end of the strip from `shell/workspace.rs:auto_panel_insert_index` must fail:
/// Assets would appear after Core Status instead of between Report and Core Status in a repaired
/// Auto tab strip.
#[test]
fn auto_surface_reveal_preserves_the_shared_panel_order() {
    let mut topology = DockTopologyByName::tab_preset(["ChartTabs", "Report", "CoreStatus", "Log"]);

    ensure_auto_topology_contains_panel(&mut topology, "Assets");

    assert_eq!(
        topology.panel_names(),
        vec![
            "ChartTabs".to_string(),
            "Report".to_string(),
            "Assets".to_string(),
            "CoreStatus".to_string(),
            "Log".to_string(),
        ],
        "Assets must be inserted after Report and before Core Status"
    );
}

/// Removing the duplicate-name guard in `shell/workspace.rs:ensure_auto_topology_contains_panel`
/// must fail: repeated reveal notifications would create duplicate Assets tabs in the saved Auto
/// topology.
#[test]
fn repeated_auto_surface_reveal_does_not_duplicate_the_panel() {
    let mut topology = DockTopologyByName::tab_preset(["ChartTabs", "Report"]);

    ensure_auto_topology_contains_panel(&mut topology, "Assets");
    ensure_auto_topology_contains_panel(&mut topology, "Assets");

    assert_eq!(
        topology
            .panel_names()
            .iter()
            .filter(|name| name.as_str() == "Assets")
            .count(),
        1,
        "a second reveal must leave the repaired topology unchanged"
    );
}

/// Skipping split children in `shell/workspace.rs:insert_auto_panel_name` must fail: an omitted
/// Assets surface would be added beside Orders rather than to the upper operational tab strip.
#[test]
fn auto_surface_reveal_repairs_the_upper_tabs_before_the_orders_leaf() {
    let mut topology = DockTopologyByName {
        center: DockTopologyNode::Split {
            horizontal: false,
            items: vec![
                DockTopologyNode::Tabs {
                    names: ["ChartTabs", "Report", "CoreStatus", "Log"]
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                },
                DockTopologyNode::Panel {
                    name: "Orders".to_string(),
                },
            ],
            sizes: vec![None, Some(260.0 + 4.0 * crate::design::TABLE_ROW_H)],
        },
        left: None,
        right: None,
        bottom: None,
    };

    ensure_auto_topology_contains_panel(&mut topology, "Assets");

    let DockTopologyNode::Split { items, .. } = &topology.center else {
        panic!("the repaired Auto topology must retain its vertical split");
    };
    let DockTopologyNode::Tabs { names } = &items[0] else {
        panic!("Assets must be repaired into the upper tab strip");
    };
    assert_eq!(
        names,
        &[
            "ChartTabs".to_string(),
            "Report".to_string(),
            "Assets".to_string(),
            "CoreStatus".to_string(),
            "Log".to_string(),
        ]
    );
    assert_eq!(
        items[1],
        DockTopologyNode::Panel {
            name: "Orders".to_string(),
        },
        "Orders must remain the lower leaf, not become an Assets sibling"
    );
}

/// Reordering the saved-topology lookup after the fallback or restoring default-only tab
/// activation in `Shell::apply_workspace_mode` must fail: a valid user layout would be reset, or a
/// returning Auto session would ignore its independent persisted tab preference.
#[test]
fn saved_auto_topology_and_tab_preference_are_applied_independently() {
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
    let preference = auto_branch
        .find("backend.auto_workspace_tab(&self.group)")
        .expect("the group Auto tab preference must be read independently");
    let activation = auto_branch
        .find("dock.activate_panel_by_name(&active_panel")
        .expect("every Auto application must activate the resolved preference");
    assert!(saved < fallback && fallback < preference && preference < activation);
    assert!(!auto_branch.contains("is_default_topology"));
}

/// Removing Alerts from the Classic-only helper or accepting any excluded surface in
/// `auto_workspace_tab_is_eligible` must fail: restart would restore an unavailable Auto tab.
#[test]
fn only_auto_top_strip_surfaces_are_persistable() {
    let classic_only = auto_classic_only_panel_names()
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        classic_only,
        std::collections::HashSet::from(["News", "Alerts"])
    );
    for eligible in [
        "ChartTabs",
        "Report",
        "Assets",
        "CoreStatus",
        "Log",
        "Detects",
    ] {
        assert!(auto_workspace_tab_is_eligible(eligible), "{eligible}");
    }
    for ineligible in ["Orders", "Unknown", ""] {
        assert!(!auto_workspace_tab_is_eligible(ineligible), "{ineligible}");
    }
    for ineligible in classic_only {
        assert!(!auto_workspace_tab_is_eligible(ineligible), "{ineligible}");
    }
}

/// Returning either stale Classic-only name from `resolved_auto_workspace_tab` must fail: Auto
/// would reveal a suspended panel instead of Report, while a valid saved choice must remain intact.
#[test]
fn stale_auto_tab_values_fall_back_without_replacing_valid_choices() {
    assert_eq!(resolved_auto_workspace_tab(Some("Assets")), "Assets");
    for panel_name in auto_classic_only_panel_names() {
        assert_eq!(resolved_auto_workspace_tab(Some(panel_name)), "Report");
    }
    assert_eq!(resolved_auto_workspace_tab(Some("future-panel")), "Report");
    assert_eq!(resolved_auto_workspace_tab(None), "Report");
}

/// Removing the failed-activation branch from
/// `shell/workspace.rs:auto_workspace_activation_fallback` must fail: a saved eligible panel that
/// is absent from the actual topology would leave Auto on an arbitrary tab instead of Report.
#[test]
fn absent_saved_eligible_tab_falls_back_to_report_without_rewriting_it() {
    let saved = resolved_auto_workspace_tab(Some("Assets"));
    let topology = DockTopologyByName::tab_preset(["ChartTabs", "Report"]);
    let activated = topology.panel_names().iter().any(|name| name == saved);

    assert!(
        !activated,
        "the independent topology intentionally omits Assets"
    );
    assert_eq!(
        auto_workspace_activation_fallback(activated),
        Some("Report")
    );
    assert_eq!(
        saved, "Assets",
        "presence fallback must not rewrite the saved value"
    );

    let source = include_str!("../workspace.rs");
    let auto_branch = source
        .split("WorkspaceMode::AutoTrading =>")
        .nth(1)
        .and_then(|tail| tail.split("WorkspaceMode::Classic =>").next())
        .expect("workspace mode application must retain a bounded Auto branch");
    let guard = auto_branch
        .find("begin_auto_topology_application()")
        .expect("fallback must begin inside the topology guard");
    let fallback = auto_branch
        .find("auto_workspace_activation_fallback(activated)")
        .expect("failed activation must resolve a presence fallback");
    let report = auto_branch
        .find("dock.activate_panel_by_name(fallback, window, dock_cx)")
        .expect("presence fallback must immediately activate Report");
    let release = auto_branch
        .find("finish_auto_topology_application(guard_generation, cx)")
        .expect("topology guard must be released only after fallback activation");
    assert!(guard < fallback && fallback < report && report < release);
    assert!(!auto_branch.contains("set_auto_workspace_tab"));
}

/// Restoring the ready/configured fraction in `shell/workspace.rs:icon_workspace_summary` must
/// fail: a 200-core rail would exceed the accepted three-character Icon summary budget at 52 px.
#[test]
fn icon_summary_stays_bounded_for_two_hundred_cores() {
    let summary = icon_workspace_summary(200);
    assert_eq!(summary, "200");
    assert!(summary.chars().count() <= 3);

    let source = include_str!("../workspace.rs");
    let rail = source
        .split("fn workspace_rail")
        .nth(1)
        .expect("workspace rail must retain a bounded implementation");
    let rail = rail.split_whitespace().collect::<String>();
    assert!(
        rail.contains(
            "div().w_full().min_w_0().flex().justify_center().child(div().min_w_0().truncate().text_center().child(summary_text)"
        ),
        "summary text must center across the rail and clip safely inside the 52 px density"
    );
}

/// Catches clearing the topology guard in `Shell::apply_workspace_mode` before GPUI delivers
/// queued dock effects: fallback Report would overwrite an unknown saved tab and the generated
/// layout event would be persisted as if the operator changed it.
#[cfg(feature = "debug-tools")]
#[gpui::test]
fn deferred_guard_suppresses_programmatic_dock_events_until_delivery(
    cx: &mut gpui::TestAppContext,
) {
    let (source, harness) = cx.update(|cx| {
        let source = cx.new(|_| DockEventSource);
        let harness = cx.new({
            let source = source.clone();
            move |cx: &mut Context<AutoGuardHarness>| {
                cx.subscribe(&source, |this, _, event: &DockEvent, _| match event {
                    DockEvent::PanelActivated { panel_name } => {
                        if let Some(panel_name) =
                            auto_workspace_tab_to_persist(true, this.applying_topology, panel_name)
                        {
                            this.saved_tab = panel_name.to_string();
                        }
                    }
                    DockEvent::LayoutChanged => {
                        if auto_workspace_topology_is_persistable(true, this.applying_topology) {
                            this.topology_writes += 1;
                        }
                    }
                    _ => {}
                })
                .detach();
                AutoGuardHarness {
                    applying_topology: false,
                    guard_generation: 0,
                    saved_tab: "future-panel".to_string(),
                    topology_writes: 0,
                }
            }
        });
        (source, harness)
    });

    cx.update(|cx| {
        harness.update(cx, |state, cx| {
            state.guard_generation = state.guard_generation.wrapping_add(1);
            state.applying_topology = true;
            let generation = state.guard_generation;
            source.update(cx, |_, cx| {
                cx.emit(DockEvent::PanelActivated {
                    panel_name: "Report".into(),
                });
                cx.emit(DockEvent::LayoutChanged);
            });
            defer_auto_topology_guard_release(cx.entity().downgrade(), generation, cx);
        });
    });
    cx.run_until_parked();

    cx.update(|cx| {
        let state = harness.read(cx);
        assert_eq!(
            state.saved_tab, "future-panel",
            "programmatic fallback must not rewrite a stale persisted value"
        );
        assert_eq!(
            state.topology_writes, 0,
            "programmatic layout effects must not enter topology persistence"
        );
        assert!(!state.applying_topology, "the deferred guard must release");
    });

    cx.update(|cx| {
        source.update(cx, |_, cx| {
            cx.emit(DockEvent::PanelActivated {
                panel_name: "Assets".into(),
            });
            cx.emit(DockEvent::LayoutChanged);
        });
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let state = harness.read(cx);
        assert_eq!(state.saved_tab, "Assets");
        assert_eq!(state.topology_writes, 1);
    });
}

/// Catches restoring the Full-row connector, dot, gaps, or padding in
/// `shell/workspace.rs:core_rail_metrics` for Icon density: the 52 px minimum would leave no
/// readable space for even the one-character core label.
#[test]
fn icon_core_geometry_reserves_a_deterministic_label_budget() {
    let metrics = core_rail_metrics(WorkspaceRailDensity::Icon);
    let occupied = 2.0 * metrics.horizontal_padding
        + metrics.connector_width
        + metrics.dot_size
        + 2.0 * metrics.gap;
    let label_budget = AUTO_WORKSPACE_RAIL_WIDTH_MIN - occupied;
    assert!(
        label_budget >= 16.0,
        "Icon core chrome may occupy at most 36 px of the 52 px minimum, got {occupied}"
    );
}

/// Catches dropping `RailItem::Core::is_last_in_section` or marking every leaf terminal in
/// `shell/workspace.rs:append_core_section_items`: the branch stem would either continue through
/// the section boundary or break between sibling cores.
#[test]
fn core_section_shape_marks_only_the_final_leaf_terminal() {
    let mut items = Vec::new();
    append_core_section_items(&mut items, vec![rail_row(10), rail_row(20), rail_row(30)]);
    let actual = items
        .into_iter()
        .map(|item| match item {
            RailItem::Core {
                row,
                is_last_in_section,
            } => (row.core, is_last_in_section),
            _ => panic!("core section helper must append only core leaves"),
        })
        .collect::<Vec<_>>();
    assert_eq!(actual, vec![(10, false), (20, false), (30, true)]);

    let mut singleton = Vec::new();
    append_core_section_items(&mut singleton, vec![rail_row(40)]);
    assert!(matches!(
        singleton.as_slice(),
        [RailItem::Core {
            is_last_in_section: true,
            ..
        }]
    ));
}

/// Routing `DockEvent::PanelActivated` through topology persistence or removing the transition
/// guard must fail: a real eligible activation should dirty only the per-group layout preference,
/// while Auto installation and its fallback activation must not overwrite the user's choice.
#[test]
fn auto_panel_activation_has_a_narrow_guarded_persistence_path() {
    let init = include_str!("../init.rs");
    let arm = init
        .split("DockEvent::PanelActivated { panel_name } =>")
        .nth(1)
        .and_then(|tail| tail.split("DockEvent::TabContextMenu").next())
        .expect("Dock activation must have an explicit event arm");
    assert!(arm.contains("auto_workspace_tab_to_persist("));
    assert!(arm.contains("this.applying_auto_topology"));
    assert!(arm.contains("backend.set_auto_workspace_tab(&group, panel_name)"));
    assert!(arm.contains("return;"));
    assert!(!arm.contains("set_auto_dock_topology"));

    let backend = include_str!("../../backend/mod.rs");
    let setter = backend
        .split("pub(crate) fn set_auto_workspace_tab")
        .nth(1)
        .and_then(|tail| {
            tail.split("pub(crate) fn workspace_core_availability")
                .next()
        })
        .expect("Backend must retain a bounded Auto tab writer");
    assert!(setter.contains("self.layout_dirty = true"));
    assert!(!setter.contains("publish_workspace_revision"));
    assert!(!setter.contains("publish_auto_workspace_layout_revision"));
}

/// Removing either shared Classic-only exclusion in `auto_only_detached_panel_names` must fail:
/// stale detached state would recreate News or Figures inside Auto with a second panel identity.
#[test]
fn live_classic_panel_name_outranks_a_stale_detached_record() {
    let classic = vec!["ChartTabs".to_string(), "Orders".to_string()];
    let detached = vec![
        DetachedSpec::new("G1".to_string(), "Orders".to_string()),
        DetachedSpec::new("G1".to_string(), "Log".to_string()),
        DetachedSpec::new("G1".to_string(), "Log".to_string()),
        DetachedSpec::new("G1".to_string(), "News".to_string()),
        DetachedSpec::new("G1".to_string(), "Alerts".to_string()),
        DetachedSpec::new("G2".to_string(), "Assets".to_string()),
    ];

    assert_eq!(
        auto_only_detached_panel_names("G1", &classic, &detached),
        vec!["Log".to_string()],
        "only a unique detached name absent from the live Classic dock needs an Auto instance"
    );
}

/// Rebuilding either docked Classic-only panel or omitting `classic_only_panels` from restoration
/// must fail: selection, zoom, and local state survive only when each exact retained `Rc` returns.
#[test]
fn docked_classic_only_panels_are_taken_before_auto_and_restore_the_same_identities() {
    let source = include_str!("../workspace.rs");
    let mode = source
        .split("pub(super) fn apply_workspace_mode")
        .nth(1)
        .and_then(|tail| tail.split("fn sync_auto_dock_topology").next())
        .expect("workspace mode application must remain bounded");
    let names = mode
        .find("auto_classic_only_panel_names()")
        .expect("Auto entry must iterate the shared Classic-only name helper");
    let take = mode
        .find("dock.take_panel_by_name(panel_name, window, dock_cx)")
        .expect("Auto entry must extract each exact docked Classic-only identity");
    let apply_auto = mode
        .find("dock.apply_topology_by_name(")
        .expect("Auto topology must still be applied");
    let retain = mode
        .find("self.classic_only_panels = classic_only_panels")
        .expect("the extracted identities must be retained by Shell");
    let restore = mode
        .find("self.classic_only_panels.clone()")
        .expect("Classic named-layout restoration must receive the retained identity");
    let clear = mode
        .rfind("self.classic_only_panels.clear()")
        .expect("retained identities must be released only after restoration");
    assert!(
        names < take
            && take < apply_auto
            && apply_auto < retain
            && retain < restore
            && restore < clear
    );
}

/// Strip `//` line comments before a source-slicing assertion runs, so prose that merely NAMES a
/// call under discussion (e.g. an explanatory comment quoting `resize_panel_silently`) can't
/// satisfy an assertion whose actual code was deleted or swapped.
fn code_only(body: &str) -> String {
    body.lines()
        .map(|line| match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Replacing `resize_panel_silently` in `shell/workspace.rs:sync_auto_rail_width` with the emitting
/// resize API must fail: a narrow window would publish its fitted width as a new global preference
/// and shrink every other Auto workspace.
#[test]
fn window_local_rail_fitting_does_not_emit_a_preference_resize() {
    let source = include_str!("../workspace.rs");
    let body = code_only(
        source
            .split("pub(super) fn sync_auto_rail_width")
            .nth(1)
            .and_then(|tail| tail.split("pub(super) fn apply_workspace_mode").next())
            .expect("Auto rail synchronization must retain a bounded implementation"),
    );
    assert!(body.contains("resize_panel_silently"));
    assert!(!body.contains("state.resize_panel("));
}

/// Deleting the `is_resizing()` early return in `shell/workspace.rs:sync_auto_rail_width`, or
/// moving it below the stored-width read, must fail: mid-drag reconciliation would again yank the
/// rail back to the stale preference and kill the in-flight drag via `resize_panel_silently`,
/// leaving the pointer dragging nothing and mouse-up never persisting the width.
#[test]
fn auto_rail_sync_defers_to_an_in_flight_drag_before_reading_the_stored_width() {
    let source = include_str!("../workspace.rs");
    let body = code_only(
        source
            .split("pub(super) fn sync_auto_rail_width")
            .nth(1)
            .and_then(|tail| tail.split("pub(super) fn apply_workspace_mode").next())
            .expect("Auto rail synchronization must retain a bounded implementation"),
    );
    let guard = body
        .find("self.workspace_resize_state.read(cx).is_resizing()")
        .expect("mid-drag width reconciliation must be guarded before it runs");
    let stored_width_read = body
        .find("self.backend.read(cx).auto_workspace_rail_width()")
        .expect("the stored rail-width preference must still be read once the guard clears");
    assert!(
        guard < stored_width_read,
        "the in-flight-drag guard must precede the stored-width read, not follow it"
    );
}
