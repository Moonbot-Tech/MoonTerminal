//! Pure regressions for workspace scope, roster, navigation, ownership, and rail presentation.

use moon_core::config::{NO_MATCH_CORE_UID, WorkspaceMode};
use moon_core::feed::{ConnStatus, CoreStartupStatus, ExchangeId};
use moon_core::venue::CoreVenue;

use super::{
    AutoWorkspaceSurface, AutoWorkspaceSurfaceRequests, EffectiveScopeLabel, RetainedCoreScope,
    WORKSPACE_RAIL_COMPACT_MIN_WIDTH, WORKSPACE_RAIL_FULL_MIN_WIDTH, WorkspaceCoreAvailability,
    WorkspaceCoreStatus, WorkspaceFocus, WorkspaceNavigationAction, WorkspaceRailDensity,
    WorkspaceRosterInput, WorkspaceWindowState, changed_auto_workspace_rail_width,
    derive_workspace_roster, focus_workspace_owner, is_auto_overview_scope,
    plan_workspace_navigation, query_core_ids, reconcile_workspace_focus,
    resolve_auto_workspace_surface, resolve_group_scope, resolve_singleton_workspace,
    should_persist_normal_dock, should_remember_classic_trade_core,
    should_return_to_report_after_main_close, workspace_rail_density,
};

/// `workspace.rs:should_return_to_report_after_main_close` must require an actually closed last
/// Main chart; accepting one remaining chart would abandon the survivor, while accepting Classic,
/// a no-op Escape, or a hidden Main beneath Add/Custom would change established navigation.
#[test]
fn escape_returns_to_report_only_after_auto_main_becomes_empty() {
    assert!(should_return_to_report_after_main_close(
        WorkspaceMode::AutoTrading,
        true,
        true,
        0
    ));
    assert!(!should_return_to_report_after_main_close(
        WorkspaceMode::AutoTrading,
        true,
        true,
        1
    ));
    assert!(!should_return_to_report_after_main_close(
        WorkspaceMode::Classic,
        true,
        true,
        0
    ));
    assert!(!should_return_to_report_after_main_close(
        WorkspaceMode::AutoTrading,
        false,
        true,
        0
    ));
    assert!(!should_return_to_report_after_main_close(
        WorkspaceMode::AutoTrading,
        true,
        false,
        0
    ));
}

/// `workspace.rs:AutoWorkspaceSurfaceRequests::request` must replace one group's prior event and
/// `resolve_auto_workspace_surface` must consume its revision in every mode. Retaining the first
/// event would invert coalesced open/close order; sharing group state or replaying a seen event
/// would move an unrelated or rebuilt window to the wrong surface.
#[test]
fn auto_surface_requests_preserve_order_group_and_cursor() {
    let mut requests = AutoWorkspaceSurfaceRequests::default();
    requests.request("alpha", AutoWorkspaceSurface::ChartTabs);
    requests.request("beta", AutoWorkspaceSurface::Report);
    requests.request("alpha", AutoWorkspaceSurface::Report);

    let mut alpha_cursor = 0;
    assert_eq!(
        resolve_auto_workspace_surface(
            WorkspaceMode::AutoTrading,
            &mut alpha_cursor,
            requests.current("alpha")
        ),
        Some(AutoWorkspaceSurface::Report),
        "a later Escape must win over the earlier chart open"
    );
    let mut beta_cursor = 0;
    assert_eq!(
        resolve_auto_workspace_surface(
            WorkspaceMode::AutoTrading,
            &mut beta_cursor,
            requests.current("beta")
        ),
        Some(AutoWorkspaceSurface::Report),
        "alpha events must not replace beta's latest surface"
    );

    requests.request("alpha", AutoWorkspaceSurface::ChartTabs);
    assert_eq!(
        resolve_auto_workspace_surface(
            WorkspaceMode::AutoTrading,
            &mut alpha_cursor,
            requests.current("alpha")
        ),
        Some(AutoWorkspaceSurface::ChartTabs),
        "a later chart open must win over the earlier Escape"
    );
    let current = requests
        .current("alpha")
        .expect("the literal request must exist");

    let mut reconstructed_cursor = current.0;
    assert_eq!(
        resolve_auto_workspace_surface(
            WorkspaceMode::AutoTrading,
            &mut reconstructed_cursor,
            Some(current)
        ),
        None
    );

    requests.request("alpha", AutoWorkspaceSurface::ChartTabs);
    let classic_event = requests.current("alpha");
    assert_eq!(
        resolve_auto_workspace_surface(
            WorkspaceMode::Classic,
            &mut reconstructed_cursor,
            classic_event
        ),
        None
    );
    assert_eq!(reconstructed_cursor, classic_event.unwrap().0);
    assert_eq!(
        resolve_auto_workspace_surface(
            WorkspaceMode::AutoTrading,
            &mut reconstructed_cursor,
            classic_event
        ),
        None,
        "an event already observed in Classic must not replay after switching to Auto"
    );
}

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

/// `workspace.rs::is_auto_overview_scope` must retain both its Auto-mode and no-selection
/// conjuncts: dropping either would hide per-core money, leverage, and exchange caps in Classic,
/// or show one arbitrary server's figures under the Auto Overview group label.
#[test]
fn auto_overview_scope_requires_auto_mode_and_no_selected_core() {
    assert!(is_auto_overview_scope(WorkspaceMode::AutoTrading, None));
    assert!(!is_auto_overview_scope(
        WorkspaceMode::AutoTrading,
        Some(11)
    ));
    assert!(!is_auto_overview_scope(WorkspaceMode::Classic, None));
    assert!(!is_auto_overview_scope(WorkspaceMode::Classic, Some(11)));
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

/// `workspace.rs:resolve_singleton_workspace` must reject Classic even for a live focused group;
/// removing its Auto-mode gate would make Analytics and Strategies treat a Classic group as their
/// live Auto owner and retarget their selected-core actions.
#[test]
fn singleton_scope_stays_auto_only_for_a_live_classic_owner() {
    assert_eq!(
        resolve_singleton_workspace("classic", true, WorkspaceMode::Classic, Some(7), &[7]),
        None,
        "a focused Classic group supplies display membership, never an Auto singleton workspace"
    );
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
    let mut connecting = roster_input(
        44,
        "Connecting",
        "beta",
        Some(4),
        true,
        true,
        true,
        true,
        false,
    );
    connecting.connection = Some(ConnStatus::Stage("connected, init".to_string()));
    connecting.startup.current_step = Some(moon_core::feed::CoreInitStep::AuthCheck);
    let inputs = vec![
        roster_input(11, "Ready", "alpha", Some(4), true, true, true, true, true),
        roster_input(
            22, "Disabled", "alpha", None, false, true, false, true, false,
        ),
        roster_input(
            33,
            "No window",
            "beta",
            Some(2),
            true,
            true,
            true,
            false,
            true,
        ),
        connecting,
    ];
    let roster = derive_workspace_roster(&inputs, "alpha", Some(11), inputs.len());

    assert_eq!(roster.sections[0].venue, None);
    assert_eq!(
        roster.sections[1]
            .venue
            .as_ref()
            .map(crate::controls::venue_label),
        Some("Binance Futures".to_string())
    );
    assert_eq!(
        roster.sections[2]
            .venue
            .as_ref()
            .map(crate::controls::venue_label),
        Some("Bybit Futures".to_string())
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
    let connecting = rows
        .iter()
        .find(|row| row.core == 44)
        .expect("the connecting core must remain visible");
    assert_eq!(
        connecting.connection,
        Some(ConnStatus::Stage("connected, init".to_string()))
    );
    assert_eq!(
        connecting.startup.current_step,
        Some(moon_core::feed::CoreInitStep::AuthCheck)
    );
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
    exchange: Option<u8>,
    core_active: bool,
    group_active: bool,
    live_session: bool,
    live_group_window: bool,
    ready: bool,
) -> WorkspaceRosterInput {
    let connection = live_session.then_some(if ready {
        ConnStatus::Ready
    } else {
        ConnStatus::Connecting
    });
    WorkspaceRosterInput {
        fault: None,
        mode_suggestion: None,
        core,
        name: name.to_string(),
        group: group.to_string(),
        venue: exchange.map(|code| CoreVenue {
            id: ExchangeId::new(code),
            dex: String::new(),
            reported: String::new(),
        }),
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
        connection,
        startup: CoreStartupStatus::default(),
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
        fault: None,
        mode_suggestion: None,
        core,
        name: format!("Core {core}"),
        group: group.to_string(),
        status: WorkspaceCoreStatus::Ready,
        selectable: true,
        selected: false,
        connection: Some(ConnStatus::Ready),
        startup: CoreStartupStatus::default(),
    }
}

/// Deleting `workspace.rs:EffectiveCoreScope::or_configured`'s fallback branch would make an
/// all-offline group read fleet-wide money totals as its own instead of querying its configured cores.
#[test]
fn all_offline_scope_falls_back_to_its_configured_universe() {
    let session = resolve_group_scope(WorkspaceMode::Classic, None, &[], RetainedCoreScope::All)
        .with_membership_counts(0, 0);
    let configured = resolve_group_scope(
        WorkspaceMode::Classic,
        None,
        &[11, 22],
        RetainedCoreScope::All,
    )
    .with_membership_counts(2, 2);

    let chosen = session.or_configured(|| configured.clone());
    assert_eq!(chosen.ids(), &[11, 22]);
    assert_eq!(chosen.membership_total(), 2);
    assert_eq!(
        query_core_ids(chosen.ids().to_vec(), chosen.membership_total() > 0,),
        vec![11, 22]
    );
}

/// Forcing `panels/report/mod.rs:ReportPanel::query_core_uids` to always pass a true sentinel flag would
/// blank a starting group's Report even though no configured core universe exists yet.
#[test]
fn starting_scope_without_configured_cores_stays_unfiltered() {
    let session = resolve_group_scope(WorkspaceMode::Classic, None, &[], RetainedCoreScope::All)
        .with_membership_counts(0, 0);
    let configured = resolve_group_scope(WorkspaceMode::Classic, None, &[], RetainedCoreScope::All)
        .with_membership_counts(0, 0);

    let chosen = session.or_configured(|| configured.clone());
    assert_eq!(
        query_core_ids(chosen.ids().to_vec(), chosen.membership_total() > 0,),
        Vec::<u64>::new()
    );

    let report_code = include_str!("../panels/report/mod.rs")
        .lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        report_code.contains("query_core_ids(scope.ids().to_vec(), scope.membership_total() > 0)"),
        "Report must preserve the membership-total sentinel flag for a starting empty group"
    );
}

/// Deleting `workspace.rs:EffectiveCoreScope::or_configured`'s fallback branch would make startup
/// and all-offline byte-identical inputs read fleet-wide Report rows instead of the group's own rows.
#[test]
fn startup_with_configured_cores_uses_the_group_and_not_the_sentinel() {
    let session = resolve_group_scope(WorkspaceMode::Classic, None, &[], RetainedCoreScope::All)
        .with_membership_counts(0, 0);
    let configured = resolve_group_scope(
        WorkspaceMode::Classic,
        None,
        &[11, 22],
        RetainedCoreScope::All,
    )
    .with_membership_counts(2, 2);

    let chosen = session.or_configured(|| configured.clone());
    let query = query_core_ids(chosen.ids().to_vec(), chosen.membership_total() > 0);
    assert_eq!(query, vec![11, 22]);
    assert!(!query.is_empty());
    assert!(!query.contains(&NO_MATCH_CORE_UID));
}

/// Relaxing `workspace.rs:EffectiveCoreScope::or_configured` from `ids.is_empty() &&
/// membership_total == 0` to only `ids.is_empty()` would broaden an offline explicit selection.
#[test]
fn offline_explicit_selection_stays_a_no_match() {
    let session = resolve_group_scope(
        WorkspaceMode::Classic,
        None,
        &[11],
        RetainedCoreScope::Explicit(&[22]),
    )
    .with_membership_counts(1, 1);
    let configured = resolve_group_scope(
        WorkspaceMode::Classic,
        None,
        &[11, 22],
        RetainedCoreScope::All,
    )
    .with_membership_counts(2, 2);

    let chosen = session.clone().or_configured(|| configured.clone());
    assert_eq!(chosen, session);
    assert_eq!(
        query_core_ids(chosen.ids().to_vec(), chosen.membership_total() > 0,),
        vec![NO_MATCH_CORE_UID]
    );
}

/// Relaxing `workspace.rs:EffectiveCoreScope::or_configured` to fall back on every empty ID list
/// would turn a preset that hides every available core into a fleet-wide Report query.
#[test]
fn preset_hiding_every_available_core_stays_a_no_match() {
    let session = resolve_group_scope(WorkspaceMode::Classic, None, &[], RetainedCoreScope::All)
        .with_membership_counts(0, 2);
    let configured = resolve_group_scope(
        WorkspaceMode::Classic,
        None,
        &[11, 22],
        RetainedCoreScope::All,
    )
    .with_membership_counts(2, 2);

    let chosen = session.clone().or_configured(|| configured.clone());
    assert_eq!(chosen, session);
    assert_eq!(
        query_core_ids(chosen.ids().to_vec(), chosen.membership_total() > 0,),
        vec![NO_MATCH_CORE_UID]
    );
}

/// Computing `backend/mod.rs:Backend::configured_workspace_scope`'s membership total after the preset filter
/// would turn a fully hidden configured universe into an unfiltered fleet-wide Report query.
#[test]
fn fully_hidden_configured_universe_stays_a_no_match() {
    let session = resolve_group_scope(WorkspaceMode::Classic, None, &[], RetainedCoreScope::All)
        .with_membership_counts(0, 0);
    let configured = resolve_group_scope(WorkspaceMode::Classic, None, &[], RetainedCoreScope::All)
        .with_membership_counts(0, 2);

    let chosen = session.or_configured(|| configured.clone());
    assert_eq!(chosen.membership_total(), 2);
    assert_eq!(
        query_core_ids(chosen.ids().to_vec(), chosen.membership_total() > 0,),
        vec![NO_MATCH_CORE_UID]
    );
}

/// Adding `live_session` to `workspace.rs:WorkspaceCoreAvailability::is_configured_active`, or
/// dropping `core_active`, would exclude startup rows or include disabled cores in Report scope.
#[test]
fn configured_activity_ignores_liveness_and_window_but_requires_both_activity_flags() {
    assert!(availability(true, true, false, WorkspaceWindowState::Missing).is_configured_active());
    assert!(!availability(true, false, true, WorkspaceWindowState::Live).is_configured_active());
    assert!(!availability(false, true, true, WorkspaceWindowState::Live).is_configured_active());
}
