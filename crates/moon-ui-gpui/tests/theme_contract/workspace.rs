//! Static contracts for Auto workspace docking, availability, ownership, scope, and delayed action
//! authority.

use super::support::*;

/// Catches adding a second `DockArea` or deleting the addressed `ChartTabs` activation in
/// `shell/workspace.rs`; either duplicates hidden panel work or makes chart requests stay hidden.
#[test]
fn auto_workspace_keeps_one_dock_and_the_chart_route() {
    let shell = code_only(&read_src("shell/mod.rs"));
    let init = code_only(&read_src("shell/init.rs"));
    let workspace = code_only(&read_src("shell/workspace.rs"));

    assert_eq!(
        shell.matches("dock: Entity<DockArea>").count(),
        1,
        "Shell must own exactly one live DockArea"
    );
    assert_eq!(
        init.matches("DockArea::new(").count(),
        1,
        "group construction must create exactly one DockArea"
    );
    assert!(
        !workspace.contains("DockArea::new("),
        "Auto must transform the existing dock rather than construct another"
    );
    let reconcile = code_only(braced_body(&workspace, "fn reconcile_workspace_window("));
    assert!(
        reconcile.contains("activate_panel_by_name(\"ChartTabs\"")
            && reconcile.contains("addressed_group.as_deref() == Some(self.group.as_str())"),
        "only a Main-open revision addressed to this Auto group may reveal ChartTabs"
    );
}

/// Catches swapping the checked-state arms in
/// `chrome/terminal_chrome.rs:workspace_mode_selector`; the compact switch would enter Classic
/// when enabled and Auto when disabled.
#[test]
fn header_workspace_mode_uses_a_compact_toggle_with_direct_state_mapping() {
    let chrome = code_only(&read_src("chrome/terminal_chrome.rs"));
    let selector = code_only(braced_body(&chrome, "fn workspace_mode_selector("));

    assert!(
        selector.contains("MoonToggle::new(\"header-workspace-mode\")")
            && selector.contains(".label(t!(\"workspace.mode.auto\").to_string())")
            && selector.contains(".checked(auto)")
            && selector.contains(".size(MoonToggleSize::Compact)")
            && !selector.contains("MoonSegmentedControl"),
        "the header must use one labeled compact toggle instead of two mode buttons"
    );
    assert!(
        selector.contains(
            "if *checked {\n                        WorkspaceMode::AutoTrading\n                    } else {\n                        WorkspaceMode::Classic"
        ) && selector.contains("backend.set_workspace_mode(&group, mode, backend_cx)"),
        "checked must map to Auto and unchecked to Classic through the shared backend authority"
    );
}

/// Catches routing Auto `LayoutChanged` through the Classic dump or restoring the removed opaque
/// snapshot dance; an Auto edit would overwrite `docks.json` or lose independent Classic state.
#[test]
fn auto_and_classic_persist_to_separate_layout_authorities() {
    let init = code_only(&read_src("shell/init.rs"));
    let event_gate = chain_between(
        &init,
        "let auto = this.backend.read(cx).workspace_mode(&this.group)",
        "cx.observe_window_bounds",
        "dock event persistence",
    );
    let topology_at = event_gate
        .find("let topology = dock.read(cx).topology_by_name(cx)")
        .expect("Auto events must project topology without panel payload");
    let classic_dump_at = event_gate
        .find("let state = dock.read(cx).dump(cx)")
        .expect("Classic events must retain their full group-local dump");
    assert!(
        topology_at < classic_dump_at
            && event_gate[topology_at..classic_dump_at]
                .contains("backend.set_auto_dock_topology(topology, backend_cx)"),
        "Auto topology must publish and return before the Classic persistence-facing dump"
    );

    let workspace = code_only(&read_src("shell/workspace.rs"));
    let mode = code_only(braced_body(
        &workspace,
        "pub(super) fn apply_workspace_mode(",
    ));
    assert!(
        mode.contains("named_layout(cx)")
            && mode.contains("apply_topology_by_name(")
            && mode.contains("apply_named_layout(")
            && !workspace.contains("normal_dock_layout")
            && !workspace.contains("snapshot_layout()"),
        "mode transitions must use independent name-based Classic and Auto layouts"
    );
}

/// Catches relocking the Auto dock, allowing detach/close, or moving Charts back among the
/// operational tabs in `shell/workspace.rs`; Auto must stay modular inside the main window while
/// keeping every operational surface except Classic-only News and pinning Charts first.
#[test]
fn auto_dock_is_modular_attached_and_charts_first() {
    let workspace = code_only(&read_src("shell/workspace.rs"));
    let mode = code_only(braced_body(
        &workspace,
        "pub(super) fn apply_workspace_mode(",
    ));
    assert!(
        mode.contains("dock.set_layout_editable(true, dock_cx)")
            && mode.contains("dock.set_detach_allowed(false, dock_cx)")
            && mode.contains("dock.set_close_allowed(false, dock_cx)")
            && mode
                .contains("dock.set_pinned_leading_panels(vec![\"ChartTabs\".into()], dock_cx)",),
        "Auto must allow in-window dock edits while disabling detach/close and pinning Charts"
    );
    assert!(
        mode.contains("dock.set_detach_allowed(true, dock_cx)")
            && mode.contains("dock.set_close_allowed(true, dock_cx)")
            && mode.contains("dock.take_panel_by_name(\"News\", window, dock_cx)")
            && mode.contains("self.classic_only_panels.clone()"),
        "Auto must suspend docked News while Classic restores that exact retained identity"
    );

    let order = workspace
        .split("const AUTO_PANEL_ORDER")
        .nth(1)
        .and_then(|tail| tail.split("];\n").next())
        .expect("Auto panel order must remain a bounded static slice");
    assert!(
        order.find("\"ChartTabs\"").expect("Charts must be present")
            < order.find("\"Report\"").expect("Report must be present"),
        "Charts must be the strict first Auto tab before operational panels"
    );
    assert!(
        !order.contains("\"News\"") && order.contains("\"Detects\""),
        "every operational Auto surface except Classic-only News must remain available"
    );
    let preset = code_only(braced_body(
        &workspace,
        "fn default_auto_workspace_topology()",
    ));
    assert!(
        preset.contains("DockTopologyNode::Split {")
            && preset.contains("horizontal: false")
            && preset.contains(".filter(|name| *name != \"Orders\")")
            && preset.contains("name: \"Orders\".to_string()")
            && preset.contains("sizes: vec![None, Some(260.0 + 4.0 * design::TABLE_ROW_H)]"),
        "first-run Auto must keep Log in the flexible upper tabs and reserve four more rows for Orders"
    );
    assert!(
        mode.contains("resolved_auto_workspace_tab(backend.auto_workspace_tab(&self.group))")
            && mode.contains("dock.activate_panel_by_name(&active_panel, window, dock_cx)"),
        "Auto must reveal the saved eligible tab or its deterministic Report fallback"
    );
}

/// Catches gating a core on its per-core `show_window` flag or duplicating group enumeration;
/// a headless core with a live session in an existing group window must remain selectable.
#[test]
fn headless_core_uses_its_live_group_window_owner() {
    let backend = code_only(&read_src("backend/mod.rs"));
    let availability = code_only(braced_body(
        &backend,
        "pub(crate) fn workspace_core_availability(",
    ));
    assert!(
        !availability.contains("show_window"),
        "per-core window visibility must not gate Auto scope or roster selection"
    );

    let configured = code_only(braced_body(&backend, "fn group_is_configured("));
    assert!(
        configured.contains("crate::window::group_window::groups(&self.config)"),
        "workspace ownership must reuse the canonical group-window enumeration"
    );
}

/// Catches removing Auto ownership refresh from native window activation; switching group windows
/// would otherwise leave Analytics and Strategies scoped to the previously active Auto group.
#[test]
fn activating_an_auto_group_window_refreshes_singleton_ownership() {
    let init = code_only(&read_src("shell/init.rs"));
    let activation = chain_between(
        &init,
        "cx.observe_window_activation(",
        ".detach();",
        "group-window activation observer",
    );
    assert!(
        activation.contains("if this.window_active")
            && activation.contains("b.note_main_input(&group);")
            && activation.contains("b.focus_auto_workspace(&group, bcx);")
            && activation
                .matches("b.focus_auto_workspace(&group, bcx);")
                .count()
                == 1,
        "native activation must refresh Auto singleton ownership without losing Main activity"
    );
}

/// Catches either detached-panel activation or interaction omitting Auto singleton ownership; the
/// last interacted Auto group must own singleton tools.
#[test]
fn detached_panel_activation_and_activity_refresh_auto_singleton_ownership() {
    let detached = code_only(&read_src("window/detached.rs"));
    let new = code_only(braced_body(
        &detached,
        "fn new(\n        backend: Entity<Backend>",
    ));
    assert!(
        new.contains("cx.observe_window_activation(")
            && new.contains("b.note_main_input(&activation_group);")
            && new.contains("b.focus_auto_workspace(&activation_group, bcx);")
            && new
                .matches("b.focus_auto_workspace(&activation_group, bcx);")
                .count()
                == 1,
        "native detached-panel activation must attribute both activity and Auto ownership once"
    );
    let render = code_only(braced_body(&detached, "fn render(&mut self, window:"));
    assert!(
        render.contains("phase == DispatchPhase::Capture && window.is_window_active()")
            && render.contains("b.note_main_input(&group);")
            && render.contains("b.focus_auto_workspace(&group, bcx);"),
        "active detached-panel interaction must refresh Main activity and Auto ownership"
    );
}

/// Catches detached-chart native activation relying on idle-close polling, or the later activity
/// path recording only Main activity; both routes must attribute Auto singleton ownership.
#[test]
fn detached_chart_activation_and_activity_refresh_auto_singleton_ownership() {
    let host = code_only(&read_src("chart_tabs/detached_host/mod.rs"));
    let new = code_only(braced_body(&host, "pub(super) fn new("));
    let activation = chain_between(
        &new,
        "cx.observe_window_activation(",
        ".detach();",
        "detached chart activation observer",
    );
    assert!(
        activation.contains("hide_window_from_taskbar_soon(window)")
            && activation.contains("if window.is_window_active()")
            && activation.contains("b.focus_auto_workspace(&group, bcx)"),
        "native detached-chart activation must focus its Auto owner independently of idle polling"
    );

    let stack = code_only(&read_src("chart_tabs/main_stack.rs"));
    let prune = code_only(braced_body(&stack, "fn prune_idle("));
    assert!(
        prune.contains(".any(|h| h.is_active(cx).unwrap_or(false))")
            && prune.contains("b.note_main_input(&group);")
            && prune.contains("b.focus_auto_workspace(&group, bcx);"),
        "active detached chart windows must refresh Main activity and Auto ownership"
    );
}

/// Catches trusting a request's stale stored group after session reconciliation; a moved target
/// must wake/consume only in its current group, while a removed target must cancel its reveal.
#[test]
fn main_open_requests_revalidate_live_group_before_signature_and_consume() {
    let backend = code_only(&read_src("backend/mod.rs"));
    let pending = code_only(braced_body(
        &backend,
        "pub(crate) fn pending_open_main_request_for_group(",
    ));
    assert!(
        pending.contains("self.current_open_main_group().as_deref() == Some(group)")
            && pending.contains("self.open_main_request.pending_target()"),
        "read-phase routing must use the target core's current live group"
    );
    let signature = code_only(braced_body(
        &backend,
        "pub(crate) fn pending_open_main_revision_for_group(",
    ));
    assert!(
        signature.contains("self.current_open_main_group().as_deref() == Some(group)"),
        "ChartTabs signatures must wake only the target core's current group"
    );
    let take = code_only(braced_body(
        &backend,
        "pub(crate) fn take_open_main_request_if_matches(",
    ));
    let reconcile_at = take
        .find("self.reconcile_open_main_request_group()")
        .expect("consume must reconcile current session ownership");
    let take_at = take
        .find("self.open_main_request.take_if_matches(group, expected)")
        .expect("consume must retain compare-and-take semantics");
    assert!(
        reconcile_at < take_at && take.contains("cx.notify();"),
        "live ownership must reconcile before compare-and-take and wake the new owner"
    );

    let chart_tabs = code_only(&read_src("chart_tabs/mod.rs"));
    let handler = code_only(braced_body(&chart_tabs, "fn handle_open_request("));
    assert!(
        handler.contains("pending_open_main_request_for_group")
            && handler.contains("take_open_main_request_if_matches")
            && !handler.contains("open_main_request.pending_for_group")
            && !handler.contains("open_main_request.take_if_matches"),
        "ChartTabs must not bypass the live-group routing authority"
    );
    let sig = code_only(&read_src("chart_tabs/sig.rs"));
    assert!(
        sig.contains("b.pending_open_main_revision_for_group(group)")
            && !sig.contains("open_main_request.pending_revision_for_group"),
        "observer signatures must not trust stale stored routing"
    );
    let settings = code_only(&read_src("settings/apply.rs"));
    let reconciliation = chain_between(
        &settings,
        "b.session.reconcile(&b.config, reports);",
        "let rebuild = delta.needs_window_rebuild(split_changed);",
        "session/open-main reconciliation",
    );
    assert!(
        reconciliation.contains("b.reconcile_open_main_request_group()")
            && reconciliation.contains("bcx.notify();"),
        "session reconciliation must atomically retarget/cancel reveal metadata and notify"
    );
}

/// Catches startup splitting one primary close across separate registry and workspace mutations;
/// the Backend method owns removal, direct focus fallout, and one revision publication.
#[test]
fn primary_group_close_uses_one_production_workspace_transition() {
    let startup = code_only(&read_startup());
    let close = chain_between(
        &startup,
        "cx.on_window_closed(",
        ".detach();",
        "primary window close callback",
    );
    assert!(
        close.contains("b.close_group_window(closed_id, bcx)")
            && !close.contains("b.group_windows.remove(")
            && !close.contains("b.publish_workspace_window_change("),
        "startup must delegate a primary close as one production transition"
    );

    let backend = code_only(&read_src("backend/mod.rs"));
    let helper = code_only(braced_body(&backend, "pub(crate) fn close_group_window("));
    assert!(
        helper.contains("self.group_windows.remove(&group)?;")
            && helper.contains("close_workspace_owner(&mut self.workspace_focus, &group);")
            && helper
                .matches("self.publish_workspace_revision(cx);")
                .count()
                == 1,
        "the registered-close method must remove once, clear focus directly, and publish once"
    );
}

/// Catches replacing the virtualized all-config rail with an ad-hoc list or bypassing the pure
/// cross-group plan; large core fleets would regress and a click could retarget the wrong window.
#[test]
fn workspace_navigation_uses_the_shared_virtual_rail_and_owner_action() {
    let workspace = code_only(&read_src("shell/workspace.rs"));
    let rail = code_only(braced_body(&workspace, "fn workspace_rail("));
    assert!(
        rail.contains("CoreOrder::new(&backend.config)")
            && rail.contains("MoonVirtualList::new(")
            && rail.contains("derive_workspace_roster("),
        "the rail must keep canonical ordering, all-config derivation, and MoonUI virtualization"
    );
    let execute = code_only(braced_body(&workspace, "fn execute_workspace_navigation("));
    assert!(
        execute.contains("WorkspaceNavigationAction::SelectCurrent { group, core }")
            && execute.contains("select_auto_workspace_core(&group, Some(core), backend_cx)")
            && execute.contains("workspace_core_availability(&group, core)")
            && execute.contains(".is_available()")
            && execute.contains("activate_auto_workspace_core(&group, core, backend_cx)")
            && execute.contains("backend.group_windows.get(&group).copied()")
            && execute.contains("window.activate_window()"),
        "rail dispatch must revalidate its captured owner before selecting or activating a window"
    );
}

/// Catches restoring the zero-height Auto dock, a local-only rail, or terminal-group sections.
///
/// Plausible breakage: removing `dock_host`'s full cross-axis height under MoonUI `h_flex`
/// reproduces the blank body from the runtime screenshot; replacing the resizable composition with
/// a plain row removes the divider; omitting the Backend resize write stops live/restart sharing;
/// grouping from `ServerConfig::group` hides exchanges.
#[test]
fn auto_body_keeps_a_visible_dock_resizable_rail_and_exchange_sections() {
    let workspace = code_only(&read_src("shell/workspace.rs"));
    let body = code_only(braced_body(&workspace, "pub(super) fn workspace_body("));
    assert!(
        body.contains("moon_h_resizable(state_id)")
            && body.contains("with_state(&resize_state)")
            && body.contains(".on_resize(")
            && body.contains("backend.set_auto_workspace_rail_width(width, backend_cx)")
            && body.contains("workspace_rail_density(rail_width)")
            && body.contains("moon_resizable_panel()")
            && body.contains("dock_host(self.dock.clone())"),
        "Auto body must use the globally shared draggable state around the one live DockArea"
    );

    let sync = code_only(braced_body(&workspace, "fn sync_auto_rail_width("));
    assert!(
        sync.contains("self.backend.read(cx).auto_workspace_rail_width()")
            && sync.contains("state.resize_panel_silently("),
        "remote rail-width revisions must update live state without publishing a fitted width as a new preference"
    );
    let init = code_only(&read_src("shell/init.rs"));
    let bounds = chain_between(
        &init,
        "cx.observe_window_bounds(window",
        ".detach();",
        "window bounds observer",
    );
    assert!(
        bounds.contains("this.sync_auto_rail_width(window, cx)"),
        "native window and DPI changes must reconcile the live rail size"
    );

    let dock_host = code_only(braced_body(&workspace, "fn dock_host("));
    assert!(
        dock_host.contains(".h_full()") && dock_host.contains(".child(dock)"),
        "the absolute DockArea host must own full cross-axis height"
    );

    let rail = code_only(braced_body(&workspace, "fn workspace_rail("));
    assert!(
        rail.contains("core_venues()")
            && rail.contains("venues.get(&server.id).cloned()")
            && rail.contains("RailItem::Exchange {")
            && rail.contains("venue: section.venue"),
        "the rail must section canonical rows by venue identity, not by a reported caption"
    );
}

/// Catches resolving logos before the background ready edge or drawing them on core leaves;
/// either change puts blocking SVG work back on the first UI frame or repeats brand marks on every
/// core instead of using one exchange branch marker.
#[test]
fn auto_rail_prewarm_and_exchange_only_logo_contract_stays_explicit() {
    let workspace = code_only(&read_src("shell/workspace.rs"));
    let prewarm = code_only(braced_body(&workspace, "fn start_exchange_logo_prewarm("));
    assert!(
        prewarm.contains("cx.background_spawn(async { crate::media::exchange_logos::prewarm() })")
            && prewarm.contains("this.exchange_logos_ready = true")
            && prewarm.contains("cx.notify();"),
        "blocking logo prewarm must publish one Shell-owned UI ready edge"
    );

    let rail = code_only(braced_body(&workspace, "fn workspace_rail("));
    let ready = rail
        .find("if self.exchange_logos_ready")
        .expect("logo resolution must be gated by the ready edge");
    let resolve = rail
        .find("and_then(crate::media::exchange_logos::exchange_logo)")
        .expect("ready rail build must resolve each brand through the shared cache");
    let virtual_list = rail
        .find("MoonVirtualList::new(")
        .expect("the rail must remain virtualized");
    assert!(ready < resolve && resolve < virtual_list);

    let render = code_only(braced_body(&workspace, "fn render_rail_item("));
    let exchange = render
        .find("RailItem::Exchange { venue, logo }")
        .expect("exchange headings must own the resolved logo");
    let image = render[exchange..]
        .find("img(logo)")
        .map(|offset| exchange + offset)
        .expect("exchange headings must render known brand logos");
    let core = render
        .find("RailItem::Core {")
        .expect("core leaves must remain explicit");
    assert!(exchange < image && image < core);
    assert!(
        !render[core..].contains("img(")
            && workspace.matches("exchange_logos::exchange_logo").count() == 1,
        "core rows and the virtual row closure must never resolve or render exchange logos"
    );
}

/// Inventory every workspace-scoped query/cache/menu/action adapter and the three deliberate
/// aggregate exceptions, so a new direct retained-core consumer cannot bypass the authority.
#[test]
fn every_workspace_scoped_surface_uses_the_effective_authority() {
    let scoped = [
        ("panels/alerts/mod.rs", "effective_workspace_scope"),
        ("panels/core_status/mod.rs", "effective_workspace_scope"),
        ("panels/core_status/interactions.rs", "effective_scope"),
        ("panels/log/mod.rs", "effective_workspace_scope"),
        ("panels/orders/mod.rs", "effective_workspace_scope"),
        ("panels/news/mod.rs", "scope_cores(b)"),
        ("panels/detects/mod.rs", "detection_core_visible"),
        ("panels/chart/trade.rs", "workspace_action_allows_core"),
        ("panels/chart/render.rs", "workspace_action_allows_core"),
        (
            "panels/chart/render_input.rs",
            "workspace_action_allows_core",
        ),
        ("panels/assets/cache.rs", "query_cores"),
        ("panels/assets/render.rs", "scope_cores"),
        ("panels/report/mod.rs", "effective_core_ids"),
        ("panels/report/actions.rs", "effective_core_ids"),
        ("panels/report/controls.rs", "workspace_scope"),
        ("analytics/mod.rs", "analytics_workspace_scope"),
        ("analytics/toolbar.rs", "analytics_core_filter_ids"),
        ("analytics/tuner/mod.rs", "strategy_selection_visible"),
        ("analytics/tuner/save.rs", "workspace_core_visible"),
        ("analytics/purge.rs", "purge_core_visible"),
        ("analytics/tuner/list/menu.rs", "action_core_ids"),
        ("strategies/state.rs", "singleton_strategy_cores"),
        ("strategies/logic.rs", "strategy_core_is_visible"),
        ("strategies/actions.rs", "strategy_core_is_visible"),
        ("strategies/versions.rs", "selected_key"),
        ("strategies/window.rs", "workspace_allows_reveal"),
        ("strategies/tree/ui.rs", "visible_strategy_cores"),
        ("strategies/tree/dialogs.rs", "strategy_core_is_visible"),
        ("strategies/tree/dnd.rs", "action_cores_visible"),
        ("strategies/tree/moon.rs", "strategy_core_is_visible"),
    ];
    for (path, authority) in scoped {
        assert!(
            code_only(&read_src(path)).contains(authority),
            "{path} must route its Phase 4 consumer through {authority}"
        );
    }
    let tuner_save = code_only(&read_src("analytics/tuner/save.rs"));
    assert!(
        tuner_save.contains("let authority = self.capture_save_authority(&targets, cx);")
            && tuner_save.matches("save_authority_is_current(").count() >= 4
            && tuner_save.contains("save_authority_matches("),
        "Analytics tuner must retain exact authority through preview, confirmation, and async Save dispatch"
    );
    let purge = code_only(&read_src("analytics/purge.rs"));
    assert!(
        purge.matches("purge_core_visible").count() >= 4
            && purge.matches("self.guard_current(cx)?;").count() >= 7
            && purge.contains("Err(PurgeStop::ScopeMoved)"),
        "Analytics purge must revalidate its Auto core before every later read, wait, and command"
    );

    let assets = code_only(&read_src("panels/assets/mod.rs"));
    assert!(
        assets.contains("let AssetsScope::Group(group) = &self.scope else")
            && assets.contains("return None;"),
        "global Assets must remain deliberately aggregate"
    );
    let report = code_only(&read_src("panels/report/mod.rs"));
    assert!(
        report.contains("if self.standalone") && report.contains("return None;"),
        "Analytics-owned standalone Report must retain its explicit historical scope"
    );
    let detects = code_only(&read_src("panels/detects/mod.rs"));
    let ingest = code_only(braced_body(&detects, "fn ingest("));
    assert!(
        ingest.contains(".filter(|s| s.group == self.group)")
            && !ingest.contains("effective_workspace_scope"),
        "Detects ingest and cursors must stay group-wide; only presentation is scoped"
    );
}

/// Removing the mode guard from `Backend::store_classic_dock_state`, or bypassing the helper from
/// Orders/Alerts persistence, would serialize Auto topology and temporary panels into Classic.
#[test]
fn panel_payload_persistence_cannot_overwrite_classic_from_auto() {
    let backend = code_only(&read_src("backend/mod.rs"));
    let helper = code_only(braced_body(
        &backend,
        "pub(crate) fn store_classic_dock_state(",
    ));
    assert!(
        helper.contains("if self.workspace_mode(&group) == WorkspaceMode::AutoTrading {")
            && helper.contains("return false;")
            && helper.find("WorkspaceMode::AutoTrading").unwrap()
                < helper
                    .find("self.dock_states.insert(group, state);")
                    .unwrap()
    );

    for path in ["panels/orders/persist.rs", "panels/alerts/mod.rs"] {
        let source = code_only(&read_src(path));
        assert!(source.contains("b.store_classic_dock_state(group, state);"));
        assert!(!source.contains("b.dock_states.insert(group, state);"));
    }
}

/// Catches moving delayed row, menu, picker, and scan callbacks outside the live workspace guard;
/// after an Auto A-to-B switch they must refuse the captured A target before their first effect.
#[test]
fn delayed_workspace_actions_revalidate_inside_the_dispatch_path() {
    let compact = |source: &str| {
        code_only(source)
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>()
    };
    let backend = read_src("backend/mod.rs");
    let authority = compact(braced_body(
        &backend,
        "pub(crate) fn workspace_action_allows_core(",
    ));
    assert!(
        authority.contains("WorkspaceMode::AutoTrading")
            && authority.contains("effective_workspace_scope(")
            && authority.contains(".contains(core)"),
        "the shared action guard must preserve Classic/unscoped behavior and test current Auto membership"
    );

    let coin_menu = read_src("controls/coin_menu.rs");
    let menu_builder = compact(braced_body(&coin_menu, "fn build_items("));
    let menu_guard = compact(braced_body(&coin_menu, "fn workspace_action_allows_cores("));
    assert!(
        menu_builder
            .matches("workspace_action_allows_cores(")
            .count()
            >= 7
            && menu_guard.contains("backend.workspace_action_allows_core(group,*core)"),
        "every shared coin-menu mutation must revalidate all captured cores atomically"
    );

    let orders = read_src("panels/orders/table.rs");
    let stop_toggle = compact(braced_body(&orders, "fn flag_toggle_cell("));
    let stop_guard = stop_toggle
        .find("b.workspace_action_allows_core(Some(&group),core)")
        .expect("Orders stop toggle must revalidate its rendered core");
    let stop_send = stop_toggle
        .find("b.session.set_order_stop(core,uid,kind,!on)")
        .expect("Orders stop toggle must retain its command");
    let stop_overlay = stop_toggle
        .find("this.stop_overlay.insert(")
        .expect("Orders stop toggle must retain its optimistic overlay");
    assert!(
        stop_guard < stop_send && stop_send < stop_overlay,
        "Orders must authorize and send before drawing an optimistic state for a stale target"
    );

    let alerts = read_src("panels/alerts/table.rs");
    let alert_commit = compact(braced_body(&alerts, "fn commit_core("));
    let settings = compact(braced_body(&alerts, "fn settings_popover("));
    assert!(
        alert_commit.contains("workspace_action_allows_core(Some(&self.group),core)")
            && settings
                .contains("workspace_action_allows_core(Some(&toggle_group),toggle_target.core)",)
            && settings.contains("WorkspaceAuthority::Group(ctx.group.clone())"),
        "Alerts row and figure-settings callbacks must share current group authority"
    );

    let figstyle = read_src("figstyle/mod.rs");
    for writer in ["fn edit_style(", "fn edit_switch("] {
        assert!(
            compact(braced_body(&figstyle, writer)).contains("if!authority.allows(b,target)"),
            "{writer} must refuse stale Alerts-owned figure writes"
        );
    }

    let trade_log = read_src("panels/report/trade_log.rs");
    let open_log = compact(braced_body(&trade_log, "pub(super) fn open_trade_log("));
    let log_guard = compact(braced_body(&trade_log, "fn trade_log_request_is_current("));
    assert!(
        open_log.matches("trade_log_request_is_current(").count() >= 2
            && log_guard.contains("revision.read(cx).generation()")
            && log_guard
                .contains("workspace_action_allows_core(Some(&workspace.group),workspace.core)",),
        "Report trade-log must validate identity before launch and again before publishing"
    );

    let news = read_src("panels/news/mod.rs");
    let open_coin = compact(braced_body(&news, "fn open_coin("));
    assert!(
        open_coin
            .matches("workspace_action_allows_core(Some(&group),core)")
            .count()
            >= 2,
        "News direct and retained-picker navigation must revalidate the current Auto core"
    );

    let chart_tabs = compact(&read_src("chart_tabs/mod.rs"));
    assert!(
        chart_tabs.contains("this.handle_open_request(false,cx);")
            && chart_tabs.contains("this.handle_open_request(true,cx);"),
        "ChartTabs must consume startup-pending requests without activation and live requests with it"
    );
}

/// Catches restoring the Auto header popover branch or its remembered-Classic fallback in
/// `chrome/terminal_chrome.rs:core_selector`; the header would become a second server selector and
/// Overview would misleadingly display the Classic server.
#[test]
fn auto_header_core_is_a_passive_overview_aware_indicator() {
    let chrome = code_only(&read_src("chrome/terminal_chrome.rs"));
    let selector = code_only(braced_body(&chrome, "fn core_selector("));
    assert!(
        selector.contains("let auto = b.workspace_mode(group) == WorkspaceMode::AutoTrading")
            && selector.contains("b.valid_auto_workspace_core(group)")
            && selector.contains("t!(\"workspace.overview\").to_string()")
            && selector.contains(".disabled(auto)")
            && selector.contains(".caret(!auto)")
            && selector.contains("if auto {\n        return pill.into_any_element();"),
        "Auto must render a passive scope pill from its own selected core or Overview"
    );
}

/// Catches removing `flex_none` from either explicit-height chrome row; the larger Auto body can
/// then shrink the header or toolbar and make both rows jump when the mode toggle changes.
#[test]
fn fixed_header_rows_cannot_flex_shrink_between_modes() {
    let chrome = code_only(&read_src("chrome/terminal_chrome.rs"));
    let header = code_only(braced_body(&chrome, "pub fn header("));
    let toolbar_src = code_only(&read_src("controls/toolbar.rs"));
    let toolbar = code_only(braced_body(&toolbar_src, "pub fn toolbar("));
    assert!(
        header.contains(".h(design::header_height_px(cx))\n        .flex_none()"),
        "the explicit-height header must be a non-shrinking flex child"
    );
    assert!(
        toolbar.contains(".h(px(design::toolbar_height(cx)))\n        .flex_none()"),
        "the explicit-height toolbar must be a non-shrinking flex child"
    );
}
