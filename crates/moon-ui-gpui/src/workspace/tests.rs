//! Pure regressions for workspace scope, roster, navigation, ownership, and rail presentation.

use moon_core::config::WorkspaceMode;

use super::{
    EffectiveScopeLabel, RetainedCoreScope, WORKSPACE_RAIL_COMPACT_MIN_WIDTH,
    WORKSPACE_RAIL_FULL_MIN_WIDTH, WorkspaceCoreAvailability, WorkspaceCoreStatus, WorkspaceFocus,
    WorkspaceNavigationAction, WorkspaceRailDensity, WorkspaceRosterInput, WorkspaceWindowState,
    changed_auto_workspace_rail_width, derive_workspace_roster, focus_workspace_owner,
    plan_workspace_navigation, reconcile_workspace_focus, resolve_group_scope,
    resolve_singleton_workspace, should_persist_normal_dock, should_remember_classic_trade_core,
    workspace_rail_density,
};

/// Protects the shared rail-width setter from redundant revisions and unusable persisted bounds.
///
/// Plausible breakage: assigning every resize sample unconditionally creates a notify feedback
/// loop across group windows, while skipping normalization can restore an invisible or dominant
/// rail after restart.
#[test]
fn auto_rail_width_change_is_clamped_and_equality_guarded() {
    assert_eq!(changed_auto_workspace_rail_width(340.0, 340.0), None);
    // 52 and 560 are the accepted UX bounds, independent of the production constants.
    assert_eq!(changed_auto_workspace_rail_width(340.0, 1.0), Some(52.0));
    assert_eq!(
        changed_auto_workspace_rail_width(340.0, 10_000.0),
        Some(560.0)
    );
    assert_eq!(changed_auto_workspace_rail_width(340.0, f32::NAN), None);
}

/// Protects Classic mode from accidentally substituting workspace Overview or selected-core state.
///
/// Plausible breakage: returning the group universe unconditionally changes every existing panel's
/// default query on upgrade and exposes cores excluded by its retained filter.
#[test]
fn classic_scope_returns_the_retained_filter() {
    let group = [11, 22, 33];
    let retained = [33, 11, 99];
    let all = resolve_group_scope(WorkspaceMode::Classic, None, &group, RetainedCoreScope::All);
    let scope = resolve_group_scope(
        WorkspaceMode::Classic,
        Some(22),
        &group,
        RetainedCoreScope::Explicit(&retained),
    );

    assert_eq!(all.ids(), &[11, 22, 33]);
    assert_eq!(all.label(), EffectiveScopeLabel::All);
    assert_eq!(scope.ids(), &[11, 33]);
    assert!(scope.contains(11));
    assert!(!scope.contains(22));
    assert!(!scope.is_workspace_owned());
    assert_eq!(scope.label(), EffectiveScopeLabel::Selection(2));
}

/// Protects Auto Overview and selected-core scope without copying either into the local filter.
///
/// Plausible breakage: falling through to the retained set makes the rail and panels disagree;
/// assigning into that set destroys the user's Classic filter when they leave Auto mode.
#[test]
fn auto_overview_and_core_override_without_mutating_local_filter() {
    let group = [11, 22, 33];
    let retained = vec![33, 11];

    let overview = resolve_group_scope(
        WorkspaceMode::AutoTrading,
        None,
        &group,
        RetainedCoreScope::Explicit(&retained),
    );
    let selected = resolve_group_scope(
        WorkspaceMode::AutoTrading,
        Some(22),
        &group,
        RetainedCoreScope::Explicit(&retained),
    );
    let stale = resolve_group_scope(
        WorkspaceMode::AutoTrading,
        Some(99),
        &group,
        RetainedCoreScope::Explicit(&retained),
    );

    assert_eq!(overview.ids(), &[11, 22, 33]);
    assert_eq!(overview.label(), EffectiveScopeLabel::Overview);
    assert_eq!(selected.ids(), &[22]);
    assert_eq!(selected.label(), EffectiveScopeLabel::Core(22));
    assert_eq!(stale.ids(), &[11, 22, 33]);
    assert_eq!(retained, vec![33, 11]);
}

/// `workspace.rs:EffectiveCoreScope::is_auto_core` must inspect the explicit scope kind. Replacing
/// it with `ids().len() == 1` hides Report's core column in one-core Classic and Auto Overview.
#[test]
fn auto_core_identity_is_not_inferred_from_single_id_cardinality() {
    let group = [11];
    let classic = resolve_group_scope(
        WorkspaceMode::Classic,
        Some(11),
        &group,
        RetainedCoreScope::All,
    );
    let overview = resolve_group_scope(
        WorkspaceMode::AutoTrading,
        None,
        &group,
        RetainedCoreScope::All,
    );
    let selected = resolve_group_scope(
        WorkspaceMode::AutoTrading,
        Some(11),
        &group,
        RetainedCoreScope::All,
    );

    assert!(!classic.is_auto_core(), "one-core Classic keeps core_name");
    assert!(
        !overview.is_auto_core(),
        "one-core Overview keeps core_name"
    );
    assert!(
        selected.is_auto_core(),
        "selected Auto core hides core_name"
    );
}

/// Protects singleton tools from inheriting an arbitrary persisted Auto group.
///
/// Plausible breakage: scanning the mode map instead of requiring the last live focus makes
/// Analytics or Strategies jump when an unrelated group changes or restarts.
#[test]
fn singleton_scope_requires_the_last_auto_owner() {
    let missing =
        resolve_singleton_workspace("beta", false, WorkspaceMode::AutoTrading, Some(22), &[22]);
    assert_eq!(missing, None);

    let owner =
        resolve_singleton_workspace("alpha", true, WorkspaceMode::AutoTrading, Some(11), &[11])
            .expect("the last live Auto owner must scope singleton tools");
    assert_eq!(owner.group, "alpha");
    assert_eq!(owner.selected_core, Some(11));
}

/// Protects active detached-window input from repeatedly invalidating singleton consumers.
///
/// Plausible breakage: assigning the owner unconditionally on every mouse event increments the
/// workspace revision continuously and causes Analytics/Strategies refresh spam.
#[test]
fn repeated_activity_in_the_same_workspace_does_not_republish_owner() {
    let mut focus = None;

    assert!(focus_workspace_owner(&mut focus, "alpha"));
    assert!(!focus_workspace_owner(&mut focus, "alpha"));
    assert!(focus_workspace_owner(&mut focus, "beta"));
    assert_eq!(focus, Some(WorkspaceFocus::new("beta")));
}

/// Protects a settings rebuild whose focused owner is reopened after another group.
///
/// Plausible breakage: excluding pre-registered Opening groups from the owner registry makes
/// singleton tools fall back to Classic when the first completed window publishes or renders.
#[test]
fn rebuild_keeps_an_owner_reopened_second_in_singleton_scope() {
    let completed = vec!["alpha".to_string()];
    let opening = vec!["beta".to_string()];
    let owner_registered = completed
        .iter()
        .chain(&opening)
        .any(|group| group == "beta");

    let scope = resolve_singleton_workspace(
        "beta",
        owner_registered,
        WorkspaceMode::AutoTrading,
        Some(22),
        &[22],
    )
    .expect("the intended second window must keep singleton ownership while Opening");
    assert_eq!(scope.group, "beta");
    assert_eq!(scope.selected_core, Some(22));
}

/// Protects one combined config/window fallback from clearing the same owner twice.
///
/// Plausible breakage: splitting config invalidation and final window removal into independent
/// owner reconciliations makes a hidden group's singleton fallback publish twice.
#[test]
fn hidden_owner_reconciles_once_after_config_and_window_changes() {
    let mut focus = Some(WorkspaceFocus::new("alpha"));

    assert!(reconcile_workspace_focus(&mut focus, false));
    assert_eq!(focus, None);
    assert!(!reconcile_workspace_focus(&mut focus, false));
}

/// Protects exchange grouping plus inventory visibility and click safety for unavailable cores.
///
/// Plausible breakage: grouping by `WorkspaceRosterInput::group` restores the terminal-group
/// headings seen in the broken Auto rail; filtering disabled/windowless rows hides inventory, while
/// marking them selectable routes a click into a group with no owning window.
#[test]
fn roster_groups_reported_exchanges_and_keeps_unavailable_rows() {
    let inputs = vec![
        roster_input(
            11,
            "Ready",
            "alpha",
            Some("Binance Futures"),
            true,
            true,
            true,
            true,
            true,
        ),
        roster_input(
            22, "Disabled", "alpha", None, false, true, false, true, false,
        ),
        roster_input(
            33,
            "No window",
            "beta",
            Some("Bybit Futures"),
            true,
            true,
            true,
            false,
            true,
        ),
        roster_input(
            44,
            "Connecting",
            "beta",
            Some("Binance Futures"),
            true,
            true,
            true,
            true,
            false,
        ),
    ];
    let roster = derive_workspace_roster(&inputs, "alpha", Some(11));

    assert_eq!(roster.sections[0].exchange, None);
    assert_eq!(
        roster.sections[1].exchange.as_deref(),
        Some("Binance Futures")
    );
    assert_eq!(
        roster.sections[2].exchange.as_deref(),
        Some("Bybit Futures")
    );
    assert_eq!(
        roster.sections[1]
            .rows
            .iter()
            .map(|row| row.core)
            .collect::<Vec<_>>(),
        vec![11, 44],
        "exchange grouping must preserve canonical input order inside each exchange"
    );
    assert_eq!(roster.summary.configured, 4);
    assert_eq!(roster.summary.ready, 1);
    assert_eq!(roster.summary.problem, 2);
    assert!(!roster.overview_selected);
    let rows: Vec<_> = roster
        .sections
        .iter()
        .flat_map(|section| &section.rows)
        .collect();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].status, WorkspaceCoreStatus::Disabled);
    assert!(!rows[0].selectable);
    assert_eq!(rows[1].status, WorkspaceCoreStatus::Ready);
    assert!(rows[1].selectable);
    assert_eq!(rows[3].status, WorkspaceCoreStatus::Unavailable);
    assert!(!rows[3].selectable);
}

/// Protects the GPUI construction interval from looking like a missing owning group window.
///
/// Plausible breakage: requiring only a completed `group_windows` handle makes a restored Auto
/// selection fall back to Overview while `open_window` is constructing the first Shell.
#[test]
fn availability_accepts_opening_owner_without_accepting_a_missing_window() {
    let opening = availability(true, true, true, WorkspaceWindowState::Opening);
    let live = availability(true, true, true, WorkspaceWindowState::Live);
    let missing = availability(true, true, true, WorkspaceWindowState::Missing);

    assert!(opening.is_available());
    assert!(live.is_available());
    assert!(!missing.is_available());
}

/// Protects exact presentation boundaries for the persisted draggable rail width.
///
/// Plausible breakage: changing either comparison from inclusive to exclusive hides server names
/// or visible failure text at the exact width where the corresponding presentation should fit.
#[test]
fn sidebar_width_boundaries_keep_content_usable() {
    assert_eq!(
        workspace_rail_density(WORKSPACE_RAIL_FULL_MIN_WIDTH),
        WorkspaceRailDensity::Full
    );
    assert_eq!(
        workspace_rail_density(WORKSPACE_RAIL_FULL_MIN_WIDTH - 0.5),
        WorkspaceRailDensity::Compact
    );
    assert_eq!(
        workspace_rail_density(WORKSPACE_RAIL_COMPACT_MIN_WIDTH - 0.5),
        WorkspaceRailDensity::Icon
    );
}

/// Protects the normal dock layout from ephemeral Auto tab activation and ordering events.
///
/// Plausible breakage: persisting Auto layout changes overwrites `docks.json`, so leaving Auto no
/// longer restores the user's split topology.
#[test]
fn auto_dock_event_policy_preserves_normal_state() {
    assert!(should_persist_normal_dock(WorkspaceMode::Classic));
    assert!(!should_persist_normal_dock(WorkspaceMode::AutoTrading));
}

/// Protects rail navigation from retargeting panel instances or stale callbacks to another group.
///
/// Plausible breakage: omitting the same-group identity lets a core moved after render select its
/// new group without activating that owner; returning the same-group action for every row shows a
/// foreign core beneath the current window.
#[test]
fn cross_group_core_selection_targets_the_owner_window() {
    let same = selectable_row(11, "alpha");
    let cross = selectable_row(22, "beta");
    assert_eq!(
        plan_workspace_navigation("alpha", &same),
        Some(WorkspaceNavigationAction::SelectCurrent {
            group: "alpha".to_string(),
            core: 11,
        })
    );
    assert_eq!(
        plan_workspace_navigation("alpha", &cross),
        Some(WorkspaceNavigationAction::ActivateGroup {
            group: "beta".to_string(),
            core: 22,
        })
    );

    let mut disabled = cross;
    disabled.selectable = false;
    assert_eq!(plan_workspace_navigation("alpha", &disabled), None);
}

/// Protects the durable Classic header core from every Auto-mode producer, including chart opens.
///
/// Plausible breakage: treating Auto as writable lets `set_main_chart_target` replace the saved
/// Classic core when an on-demand chart opens.
#[test]
fn auto_producers_cannot_remember_a_classic_trade_core() {
    assert!(should_remember_classic_trade_core(WorkspaceMode::Classic));
    assert!(!should_remember_classic_trade_core(
        WorkspaceMode::AutoTrading
    ));
}

/// Build one literal roster input for focused policy tests.
fn roster_input(
    core: u64,
    name: &str,
    group: &str,
    exchange: Option<&str>,
    core_active: bool,
    group_active: bool,
    live_session: bool,
    live_group_window: bool,
    ready: bool,
) -> WorkspaceRosterInput {
    WorkspaceRosterInput {
        core,
        name: name.to_string(),
        group: group.to_string(),
        exchange: exchange.map(str::to_string),
        availability: availability(
            group_active,
            core_active,
            live_session,
            if live_group_window {
                WorkspaceWindowState::Live
            } else {
                WorkspaceWindowState::Missing
            },
        ),
        ready,
    }
}

/// Build one availability record from independent literal lifecycle facts.
fn availability(
    group_active: bool,
    core_active: bool,
    live_session: bool,
    window: WorkspaceWindowState,
) -> WorkspaceCoreAvailability {
    WorkspaceCoreAvailability {
        group_active,
        core_active,
        live_session,
        window,
    }
}

/// Build one selectable row without coupling the navigation oracle to roster derivation.
fn selectable_row(core: u64, group: &str) -> super::WorkspaceRosterRow {
    super::WorkspaceRosterRow {
        core,
        name: format!("Core {core}"),
        group: group.to_string(),
        status: WorkspaceCoreStatus::Ready,
        selectable: true,
        selected: false,
    }
}
