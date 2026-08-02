//! The Analytics window: its reopen state, refresh throttling, busy overlay, calendar, and
//! the render root that must schedule no repaints of its own.

use super::support::*;

/// Report columns must preserve retained widths and expose their overflow through a visible bar.
///
/// Replacing `Preserve` with `Fit` would silently compress long fields again. Restoring the old
/// clamp observer would also rewrite the user's retained widths to the current viewport, so moving
/// the window back to a wider host could not recover them.
#[test]
fn report_table_uses_scrollable_preserved_widths() {
    let render = read_src("panels/report/render.rs");
    let state = read_src("panels/report/state.rs");
    let widths = read_src("panels/report/widths.rs");
    let table = braced_body(&render, "pub(super) fn table_el(");

    assert!(
        table.contains(".width_policy(MoonDataTableWidthPolicy::Preserve)")
            && table.contains(".horizontal_scrollbar_visibility(MoonScrollbarVisibility::Always)"),
        "every Report host must use preserved column widths and an always-visible overflow bar"
    );
    assert!(
        state.contains("table_persist::persist(&this.backend, &this.widths_id, &state, cx)")
            && !state.contains("clamp_table_widths"),
        "column-state observation must persist exact user widths without viewport clamping"
    );
    assert!(
        !widths.contains("plan_clamp") && !widths.contains("clamp_table_widths"),
        "the superseded width-budget clamp must not remain as a second sizing policy"
    );
}

/// Reusing the old Report persistence context must fail this assertion; a saved standalone `:win`
/// column set would otherwise override the migrated defaults and keep Profit % invisible.
#[test]
fn report_layout_uses_the_versioned_context_for_dock_and_window() {
    let state = read_src("panels/report/state.rs");
    assert_eq!(
        state.matches("ctx_id(\"report-table-v2\"").count(),
        2,
        "both dock and standalone contexts must move together"
    );
    assert!(!state.contains("ctx_id(\"report-table\""));
}

/// Strategy rows must keep both navigation paths and the Report filter in the wrapping controls.
///
/// Removing `.children(strategy_button)` is a plausible compile-clean edit that would hide the
/// Strategies launcher; removing the double-click arm would make rows select-only again. Replacing
/// the grouped `MoonCombobox` with the old eager `MoonDropdown` recreates the user-visible freeze
/// at 1,000 items. Removing the existing-window `apply_scope` call silently creates stale or
/// duplicate Reports.
/// Removing saved-bound restoration or its observer resets moved Report windows after reopening.
///
/// Returns:
///     Nothing; the source-level binary UI contract is asserted.
#[test]
fn strategy_rows_open_scoped_reports_and_live_strategy_editor() {
    let table = read_src("analytics/tuner/list/table.rs");
    let tuner = read_src("analytics/tuner/mod.rs");
    let report = read_src("panels/report/render.rs");
    let report_actions = read_src("panels/report/actions.rs");
    let report_controls = read_src("panels/report/controls.rs");
    let report_query = read_src("panels/report/query.rs");
    let report_state = read_src("panels/report/state.rs");
    let report_strategy_filter = read_src("panels/report/strategy_filter.rs");
    let report_window = read_src("panels/report/window.rs");
    let row = braced_body(&table, "fn strategy_row(");
    for needle in [
        ".children(strategy_button)",
        "icons/bot.svg",
        "app.stop_propagation()",
        "super::RowClick::OpenReport",
        "this.open_strategy_report",
    ] {
        assert!(
            row.contains(needle),
            "`strategy_row` must retain {needle:?} for the two row navigation actions"
        );
    }
    let live = braced_body(&tuner, "fn open_live_strategy(");
    assert!(
        live.contains("self.live_strategy_target(key, cx)")
            && live.contains("crate::strategies::open_goto("),
        "the editor action must recheck the live pair before using the existing goto path"
    );
    let live_gate = braced_body(&tuner, "fn live_strategy_target(");
    for needle in [
        "strategy_id == 0",
        ".store()",
        ".core(core_uid)",
        "strategy.id == live_id",
        ".then_some((core_uid, live_id))",
    ] {
        assert!(
            live_gate.contains(needle),
            "the editor button must retain exact live/manual gating via {needle:?}"
        );
    }
    let report_scope = braced_body(&tuner, "fn open_strategy_report(");
    for needle in [
        "list::inclusive_report_bounds(query.from, query.to)",
        "core_uid,",
        "strategy_id,",
        "strategy_name: name",
        "date_from,",
        "date_to,",
        "side: query.side",
        "emulator: query.emulator",
    ] {
        assert!(
            report_scope.contains(needle),
            "Analytics-to-Report scope must retain {needle:?}"
        );
    }
    let render = braced_body(&report, "fn render(");
    let strategy = render
        .find(".child(self.strategy_combo(cx))")
        .expect("Report controls must include the strategy selector");
    let wrapping = render
        .find(".flex_wrap()")
        .expect("Report controls must retain narrow-width wrapping");
    assert!(
        wrapping < strategy && !render.contains(".overflow_x_scroll()"),
        "the strategy selector must stay inside the wrapping filter row without horizontal scroll"
    );
    let strategy_combo = braced_body(&report_controls, "pub(super) fn strategy_combo(");
    for needle in [
        "MoonCombobox::new(&self.strategy_select)",
        // A combobox paints itself as an input — its own fill, focus ring, label size and a height
        // that ignores the Font slider's delta. Standing in a row of MoonDropdown filters it must
        // ask MoonUI for the button look instead of re-deriving it here.
        ".trigger_variant(MoonButtonVariant::Soft)",
        ".trigger_size(MoonButtonSize::Action)",
        ".menu_chrome(MoonComboboxMenuChrome::Menu)",
        ".font_family(design::mono())",
        ".cleanable(false)",
        ".render_trigger(",
        "crate::controls::CORE_COMBO_TRIGGER_W",
        "report.strategies_n",
        "report.search_strategies",
    ] {
        assert!(
            strategy_combo.contains(needle),
            "the large strategy selector must retain MoonUI's virtual searchable path via {needle:?}"
        );
    }
    assert!(
        !strategy_combo.contains("self.strategies.iter()")
            && !strategy_combo.contains("MoonDropdown::new"),
        "opening Reports must not rebuild an eager MoonMenuItem per strategy"
    );
    for needle in [
        "ReportStrategyChoice::All",
        "ReportStrategyChoice::Core",
        "ReportStrategyChoice::Exact",
        "fn on_will_change(",
        "available_core_indices(core_uid)",
        "self.search.replace(query.to_string())",
        "selected_available_by_core",
        "source_rows: Option<Vec<usize>>",
    ] {
        assert!(
            report_strategy_filter.contains(needle),
            "the strategy delegate must retain grouped exact multi-selection via {needle:?}"
        );
    }
    assert!(
        report_state.contains("MoonComboboxState::new(")
            && report_state.contains(".multiple(true)")
            && report_state.contains(".searchable(true)"),
        "Report strategy state must retain MoonUI's grouped virtualized multi-select engine"
    );
    let sync_select = braced_body(&report_state, "pub(super) fn flush_strategy_select_sync(");
    for needle in [
        "if self.strategy_select_items_dirty",
        "self.strategy_catalog =",
        ".selected_indices(self.selected_strategies.as_ref())",
        "ReportStrategyDelegate::catalog(",
        "ReportStrategyDelegate::unfiltered(",
        "select.set_items(unfiltered, window, select_cx)",
        "select.set_selected_indices(selected, window, select_cx)",
        "select.set_items(filtered, window, select_cx)",
    ] {
        assert!(
            sync_select.contains(needle),
            "programmatic Report changes must synchronize the retained selector via {needle:?}"
        );
    }
    assert!(
        braced_body(&report_state, "pub(crate) fn apply_scope(")
            .contains("self.queue_strategy_select_sync(true, cx)")
            && report_query.contains("this.queue_strategy_select_sync(true, cx)")
            && braced_body(&report_actions, "fn reconcile_strategy_core(")
                .contains("self.queue_strategy_select_sync(false, cx)"),
        "metadata, repeated scoped opens, and core exclusion must keep widget state synchronized"
    );
    assert!(
        report_query.contains("merge_strategy_metadata(")
            && report_query.contains("this.available_strategy_keys = available"),
        "metadata refresh must preserve selected stale labels without marking them available"
    );
    let open_report = braced_body(&report_window, "pub fn open_scoped(");
    for needle in [
        "backend.report_window_view.clone()",
        "panel.apply_scope(next, window, panel_cx)",
        "window.activate_window()",
        "cx.open_window(options",
    ] {
        assert!(
            open_report.contains(needle),
            "scoped Reports must retain singleton reuse and stale-handle recovery via {needle:?}"
        );
    }
    assert!(
        open_report.contains("layout.report_window")
            && open_report.contains("saved_or_owner_display_id(")
            && open_report.contains("restored_report_bounds(")
            && open_report.contains("saved_origin_is_visible"),
        "Report geometry must restore the saved rectangle and retain the display-safe fallback"
    );
    assert!(
        !open_report.contains("cfg!(target_os = \"macos\")"),
        "saved Report origins must not bypass attached-display validation on macOS"
    );
    let standalone = braced_body(&report_state, "pub(crate) fn mark_standalone(");
    assert!(
        standalone.contains("observe_window_bounds(")
            && standalone.contains("layout.report_window")
            && standalone.contains("layout_dirty = true"),
        "moving or resizing the standalone Report must enter the shared layout persistence path"
    );
    let set_strategy = braced_body(&report_actions, "pub(super) fn set_strategy_choices(");
    assert!(
        set_strategy.contains("exact_strategy_selection(choices)")
            && set_strategy.contains("self.selected_strategies = selected_strategies")
            && !set_strategy.contains("self.sel_cores"),
        "multi-strategy changes must not narrow the independent core filter and self-lock the catalog"
    );
    for signature in [
        "pub(super) fn toggle_core(",
        "pub(super) fn toggle_exchange_cores(",
        "pub(super) fn filter_to_core(",
    ] {
        assert!(
            braced_body(&report_actions, signature).contains("self.reconcile_strategy_core(cx);"),
            "{signature} must clear a strategy excluded by the new core selection"
        );
    }
    let filter = braced_body(&report_query, "pub(super) fn filter(");
    assert!(
        filter.contains("closed_only: self.closed_only")
            && filter.contains("strategies: normalized_strategy_filter_keys(")
            && filter.contains("self.selected_strategies.as_ref()"),
        "rows, totals, and export must share the stale-safe exact multi-strategy filter"
    );
    assert!(
        report_query.contains("strategy_metadata_request(")
            && report_query.contains("self.last_strategy_scope.as_ref()")
            && report_query.contains("db::distinct_strategies(&snap, &scope)")
            && report_query.contains("this.last_strategy_scope = Some(strategy_scope)"),
        "strategy choices must refresh from the active non-strategy Report scope and publish its matching snapshot"
    );
}

/// Analytics tabs size from their title while the complete core/filter group wraps together.
///
/// This binary crate exposes no importable GPUI view. The source contract pins two plausible
/// visual regressions: restoring the old fixed 112px button makes a long translation touch its
/// blue background, while removing the atomic filter group clips a selector at narrow widths or
/// strands the selected core name on the tabs' line.
#[test]
fn analytics_tabs_and_core_caption_follow_their_content() {
    let toolbar = read_src("analytics/toolbar.rs");
    let core_combo = read_src("controls/core_combo.rs");
    let body = braced_body(&toolbar, "pub(super) fn tabs_bar(");
    for needle in [
        "let title = t.title();",
        "design::ui_text_width(cx, &title, 10.5, 400.0, true)",
        "design::ui_value(cx, 20.0)",
        ".max(design::ui_value(cx, 72.0))",
        ".width(tab_width)",
        ".label(title)",
    ] {
        assert!(
            body.contains(needle),
            "`tabs_bar` must contain {needle:?} so every localized tab keeps measured padding"
        );
    }
    assert!(
        !body.contains(".width(112.0)"),
        "the old fixed width must not override content-driven tab sizing"
    );
    for needle in [
        ".min_h(design::fit_h_px(cx, 34.0, 13.0, 10.5))",
        ".flex_wrap()",
        ".gap_x(design::ui_px(cx, TOOLBAR_GAP))",
        ".gap_y(design::ui_px(cx, 4.0))",
        ".pt(design::ui_px(cx, 5.0))",
        ".pb(design::ui_px(cx, 4.0))",
    ] {
        assert!(
            body.contains(needle),
            "`tabs_bar` must contain {needle:?} so a wrapped header retains compact geometry"
        );
    }
    assert!(
        !body.contains(".h(design::fit_h_px(cx, 34.0, 13.0, 10.5))"),
        "a fixed row height would clip the wrapped filter group"
    );
    assert!(
        toolbar.contains("crate::controls::CORE_COMBO_TRIGGER_W")
            && core_combo.contains(".trigger_width_scaled(CORE_COMBO_TRIGGER_W)")
            && body.contains("let action_trigger_scale = design::font_value(cx, 10.5) / 10.5;")
            && body.contains("* action_trigger_scale"),
        "the responsive floor and shared core dropdown must use the same width and Action scaling"
    );

    let selectors_start = body
        .find("let selectors = h_flex()")
        .expect("the four selectors must remain one nested row");
    let filters_start = body
        .find("let filters = h_flex()")
        .expect("the selected-core caption and selectors must remain one wrapping unit");
    let row_child = body
        .find("row.child(filters)")
        .expect("the filter unit must remain a direct wrapping-row child");
    assert!(
        selectors_start < filters_start && filters_start < row_child,
        "selectors must be built before the atomic filter unit is added to the wrapping row"
    );
    let selectors = &body[selectors_start..filters_start];
    for needle in [
        ".gap(design::ui_px(cx, TOOLBAR_GAP))",
        ".child(self.core_combo(cx))",
        ".child(self.side_combo(cx))",
        ".child(self.kind_combo(cx))",
        ".child(self.metric_combo(cx))",
    ] {
        assert!(
            selectors.contains(needle),
            "the atomic selector row must contain {needle:?}"
        );
    }
    let filters = &body[filters_start..row_child];
    for needle in [
        ".flex_1()",
        ".min_w(px(filters_min_w))",
        ".min_w_0()",
        ".truncate()",
        ".text_right()",
        ".child(selectors)",
    ] {
        assert!(
            filters.contains(needle),
            "the filter unit must contain {needle:?} so its caption stays beside its selectors"
        );
    }
}

/// The metric popup must fit its localized items without widening the compact active-unit trigger.
///
/// Restoring the fixed 120px menu clips the Russian `PnL` label, while replacing the trigger width
/// with the fitted menu width needlessly consumes the responsive Analytics toolbar.
#[test]
fn analytics_metric_menu_fits_its_localized_labels() {
    let toolbar = read_src("analytics/toolbar.rs");
    let metric_combo = braced_body(&toolbar, "fn metric_combo(");

    assert!(
        metric_combo.contains(".trigger_width_scaled(METRIC_TRIGGER_W)")
            && metric_combo.contains(".fit_menu_width(120.0, 240.0)"),
        "the metric dropdown must keep a compact trigger and fit the popup to its localized items"
    );
    assert!(
        !metric_combo.contains(".menu_width_scaled(120.0)"),
        "the clipped fixed-width metric popup must not return"
    );
}

/// Protects the process boundary of Analytics UI memory.
///
/// The plausible edit is rebuilding `AnalyticsView` from hard-coded defaults or moving its
/// snapshot into `WindowLayout`. Closing the tool window would then forget the tab/filter, or a
/// full application restart would incorrectly retain them.
#[test]
fn analytics_reopen_state_is_process_lifetime_only() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let main = fs::read_to_string(root.join("main.rs")).unwrap();
    let startup = fs::read_to_string(root.join("startup.rs")).unwrap();
    let analytics = fs::read_to_string(root.join("analytics").join("mod.rs")).unwrap();
    let toolbar = fs::read_to_string(root.join("analytics").join("toolbar.rs")).unwrap();
    let tuner = fs::read_to_string(root.join("analytics").join("tuner").join("mod.rs")).unwrap();
    let ui_session = fs::read_to_string(root.join("ui_session.rs")).unwrap();
    let layout = fs::read_to_string(
        root.parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("moon-core")
            .join("src")
            .join("config")
            .join("layout.rs"),
    )
    .unwrap();

    assert!(
        main.contains("ui_session: UiSessionState,")
            && startup.contains("ui_session: UiSessionState::default(),"),
        "Backend must create one process-lifetime UiSessionState at application startup"
    );
    assert!(
        analytics.contains("let session = backend.read(cx).ui_session.analytics.clone();")
            && analytics.contains("sel_cores: session.sel_cores")
            && analytics.contains("session.strat_mode")
            && analytics.contains("b.ui_session.analytics.sel_cores = selected;")
            && toolbar.contains("b.ui_session.analytics.tab = t;")
            && tuner.contains("backend.ui_session.analytics.strat_mode = mode;"),
        "Analytics construction and all reopen choices must share the Backend UI-session snapshot"
    );
    let tab_init = analytics
        .find("tab: if probe {")
        .expect("Analytics must retain the probe-first tab branch");
    let tab_init = &analytics[tab_init..];
    let probe_tab = tab_init
        .find("Tab::Strategies")
        .expect("probe mode must still open Strategy Tuning");
    let remembered_tab = tab_init
        .find("session.tab")
        .expect("normal mode must restore the remembered tab");
    assert!(
        probe_tab < remembered_tab,
        "MOON_ANALYTICS_PROBE must override the remembered normal-session tab"
    );
    assert!(
        !ui_session.contains("Serialize")
            && !layout.contains("UiSessionState")
            && !layout.contains("AnalyticsSessionState"),
        "process-lifetime UI state must not enter the serialized WindowLayout"
    );
    // The undated-trades notice is process-lifetime state: it starts collapsed and cannot be
    // persisted in a way that suppresses the only warning about omitted money across restarts.
    assert!(
        !layout.contains("analytics_undated_hidden_n")
            && !toolbar.contains("b.layout.analytics_undated"),
        "the undated-trades notice must not be persisted to layout.toml"
    );
    // The DEFAULT is asserted by `analytics::toolbar::tests`, which reads
    // `AnalyticsSessionState::default()` directly; here only the wiring is pinned, so switching
    // the session state to `#[derive(Default)]` stays an innocent refactor.
    assert!(
        analytics.contains("undated_expanded: session.undated_expanded,")
            && toolbar.contains("b.ui_session.analytics.undated_expanded"),
        "the notice's open state must live on the UI-session snapshot"
    );
}

/// Liquidation attribution has no user-facing switch.
///
/// Plausible edit this catches: adding a checkbox, layout key, or environment gate would let two
/// installations assign the same liquidation differently.
#[test]
fn liquidation_attribution_has_no_user_switch() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let toolbar = fs::read_to_string(root.join("analytics").join("toolbar.rs")).unwrap();
    let analytics = fs::read_to_string(root.join("analytics").join("mod.rs")).unwrap();
    let layout = fs::read_to_string(
        root.parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("moon-core")
            .join("src")
            .join("config")
            .join("layout.rs"),
    )
    .unwrap();
    assert!(
        !toolbar.contains("an-attr-liq") && !toolbar.contains("attr_liq"),
        "the Analytics toolbar must carry no liquidation-attribution switch"
    );
    assert!(
        !analytics.contains("attr_liq") && !layout.contains("analytics_attribute_liq"),
        "liquidation attribution must not be gated by view or persisted state"
    );
    // A field-level gate is caught more strongly than any grep could: `query/tests.rs::q()` builds
    // `Query { .. }` field by field with no `..Default::default()`, so a new field fails to
    // COMPILE, and `a_liquidation_is_attributed_to_the_strategy_named_in_the_row` proves
    // attribution happens with no flag in sight.
}

/// Automatic report reads must reuse stale Analytics content without dimming the whole window.
///
/// The plausible regression is replacing `show_overlay` with `true` at any `spawn_db` call below:
/// every trade burst then raises the delayed busy overlay and Strategy Tuning visibly flashes
/// every 5-10 seconds even though its old snapshot is still safe to display.
#[test]
fn automatic_analytics_refresh_keeps_the_busy_overlay_hidden() {
    for (rel, signatures) in [
        (
            "analytics/mod.rs",
            &["fn reload_summary(", "fn reload_strategy_base("][..],
        ),
        (
            "analytics/calendar/mod.rs",
            &["fn reload_calendar_inner("][..],
        ),
        (
            "analytics/tuner/filter/mod.rs",
            &["fn reload_tuner_inner("][..],
        ),
        (
            "analytics/tuner/time/mod.rs",
            &["fn reload_time_inner("][..],
        ),
        (
            "analytics/tuner/coins/load.rs",
            &["fn reload_coins_inner("][..],
        ),
    ] {
        let source = read_src(rel);
        for signature in signatures {
            let after_signature = source
                .split_once(signature)
                .unwrap_or_else(|| panic!("{rel} must contain {signature}"))
                .1;
            let spawn_args = after_signature
                .split_once("self.spawn_db(")
                .unwrap_or_else(|| panic!("{rel}: {signature} must start its database read"))
                .1
                .trim_start();
            assert!(
                spawn_args.starts_with("show_overlay,"),
                "{rel}: {signature} must pass the explicit presentation policy to spawn_db"
            );
        }
    }

    let analytics = read_src("analytics/mod.rs");
    assert!(
        analytics.contains("this.reload_axis_after_report(this.strat_mode, show_overlay, cx);"),
        "the Strategy base-to-axis chain must retain the original manual/background overlay policy"
    );
}

/// `analytics/mod.rs:reload_strategy_base` must retain the current Strategies snapshot while an
/// automatic report refresh is in flight or briefly fails; restoring an unconditional reset,
/// applying an automatic failure, or swapping either caller's `after_report` polarity makes the
/// list, quote selector, and trade count blink through Loading/error after a live trade or leaves
/// old-scope values visible after a manual filter change.
#[test]
fn automatic_strategy_refresh_keeps_the_visible_snapshot() {
    let analytics = read_src("analytics/mod.rs");
    let reload = braced_body(&analytics, "fn reload_strategy_base(");
    let automatic = braced_body(&analytics, "fn refresh_visible_report_data(");
    let manual = braced_body(&analytics, "fn reload(&mut self, cx: &mut Context<Self>)");
    let reset = "self.strategy_data = ProfitLoadState::default();";

    assert_eq!(
        reload.matches(reset).count(),
        1,
        "reload_strategy_base must have exactly one strategy snapshot reset"
    );
    assert!(
        reload.contains(&format!(
            "if !after_report {{\n            {reset}\n        }}"
        )),
        "only an explicit scope reload may clear the visible Strategies snapshot"
    );
    assert!(
        automatic.contains("self.reload_strategy_base(true, true, show_overlay, cx);"),
        "automatic report refresh must request same-scope snapshot preservation"
    );
    assert!(
        manual.contains("self.reload_strategy_base(false, true, true, cx)"),
        "manual scope refresh must retire values from the previous scope"
    );
    let automatic_result = chain_between(
        reload,
        "if !after_report || data_error.is_none() {",
        "this.strategy_dirty = refresh::report_result_is_stale(",
        "automatic strategy result publication",
    );
    assert!(
        automatic_result.contains("this.strategy_data.apply(data);")
            && automatic_result.contains("this.strat_core_w = None;")
            && automatic_result.contains("this.strat_visible = None;"),
        "an automatic read failure must preserve the complete visible strategy snapshot"
    );
}

/// Automatic Report refresh must never start its heavy query from a generation callback, timer,
/// constructor, or in-flight completion. Reintroducing `this.schedule_requery(cx)` in the timer or
/// completion restores the exact five-second terminal freeze while a separate Analytics window is
/// being scrolled. The focused OS-window render is the sole automatic query-start boundary.
#[test]
fn hidden_report_never_starts_its_five_second_query() {
    let query = read_src("panels/report/query.rs");
    let render = read_src("panels/report/render.rs");
    let state = read_src("panels/report/state.rs");
    let generation = braced_body(&query, "pub(super) fn requery_on_generation(");
    let schedule = braced_body(&query, "pub(super) fn schedule_requery(");
    let report_render = braced_body(&render, "fn render(&mut self, window:");
    let constructor = braced_body(&state, "pub(crate) fn new_with_scope(");

    assert!(
        generation.contains("self.generation_refresh.observe(since)")
            && generation.contains("this.generation_refresh.timer_fired(timer_token)")
            && !generation.contains("request_requery(cx)")
            && !generation.contains("schedule_requery(cx)"),
        "generation and timer paths may publish only a bounded wake edge"
    );
    assert!(
        !query.contains("this.schedule_requery(cx);")
            && schedule.contains("self.generation_refresh.query_started();")
            && schedule.contains("if this.needs_query {")
            && schedule.contains("cx.notify();\n                        return;"),
        "in-flight completion must preserve pending catch-up without restarting hidden work"
    );
    assert!(
        !constructor.contains("schedule_requery(cx)"),
        "a hidden Report constructor must defer its initial query to active render"
    );
    let active = braced_body(report_render, "if window.is_window_active()");
    assert!(
        active.contains("self.generation_refresh.take_due()")
            && active.contains("self.schedule_requery(cx);"),
        "active Report render must consume one due edge and start pending work"
    );
}

/// `analytics/calendar/mod.rs:reload_calendar_inner` must read Calendar and its undated
/// warning through one compound API; restoring the old metadata follow-up can publish
/// adjacent report generations as one visible state under continuous ingestion.
#[test]
fn calendar_refresh_keeps_visible_metadata_in_one_snapshot() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let calendar = fs::read_to_string(
        root.join("src")
            .join("analytics")
            .join("calendar")
            .join("mod.rs"),
    )
    .unwrap();
    let core_analytics = fs::read_to_string(
        root.parent()
            .unwrap()
            .join("moon-core")
            .join("src")
            .join("db")
            .join("analytics")
            .join("mod.rs"),
    )
    .unwrap();
    let calendar_data = fn_body(&core_analytics, "pub fn calendar_data(");

    assert!(
        calendar.contains("moon_core::db::analytics::calendar_data(")
            && !calendar.contains("fn reload_report_metadata(")
            && calendar_data
                .contains("period: calendar::calendar_period_from(snapshot, q, previous, hourly)")
            && calendar_data.contains("undated: undated_closes_on(snapshot, q)"),
        "Calendar cells and undated metadata must be derived from the same read snapshot"
    );
}

/// All tabs must share the one-minute core-selector cadence. Moving `distinct_cores`
/// back into the unconditional Summary or Strategies payload restores a full-table
/// grouping on every automatic refresh under continuous ingestion.
#[test]
fn analytics_core_metadata_is_throttled_across_tabs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let analytics_ui =
        fs::read_to_string(root.join("src").join("analytics").join("mod.rs")).unwrap();
    let calendar_ui = fs::read_to_string(
        root.join("src")
            .join("analytics")
            .join("calendar")
            .join("mod.rs"),
    )
    .unwrap();
    let core_analytics = fs::read_to_string(
        root.parent()
            .unwrap()
            .join("moon-core")
            .join("src")
            .join("db")
            .join("analytics")
            .join("mod.rs"),
    )
    .unwrap();
    let summary_read = fn_body(&core_analytics, "pub fn summary_data(");
    let strategy_read = fn_body(&core_analytics, "pub fn strategy_base_data(");
    let strategy_base = fn_body(&core_analytics, "fn strategy_base_on(");
    let summary = fn_body(&core_analytics, "pub(super) fn summary_on(");

    assert!(
        analytics_ui.contains("moon_core::db::analytics::summary_data(&q, read_cores)")
            && analytics_ui
                .contains("moon_core::db::analytics::strategy_base_data(&q, read_cores)")
            && calendar_ui.contains("let refresh_cores = self.core_metadata_due(cx);")
            && analytics_ui.contains("fn core_metadata_due(")
            && summary_read.contains("cores: read_cores.then(")
            && strategy_read.contains("cores: read_cores.then(")
            && !strategy_base.contains("distinct_cores")
            && summary.contains("cores: if read_cores {"),
        "Summary, Strategies, and Calendar must use the shared throttled core metadata path"
    );
}

/// The Analytics render root must remain free of repaint scheduling.
///
/// Moving the busy-overlay timer into `render` or `busy_overlay_due` makes each timer-driven frame
/// schedule its successor. `op_started` instead arms one timer on the batch's idle-to-busy
/// transition and identifies that batch at wake-up.
#[test]
fn the_analytics_render_root_schedules_no_repaints() {
    let src = read_src("analytics/mod.rs");

    // Guard the render root as well as its predicate so scheduling cannot move one line outward.
    let render = braced_body(&src, "fn render(&mut self, window: &mut Window");
    for scheduler in ["cx.spawn(", "spawn_in(", ".timer(", "on_next_frame("] {
        assert!(
            !render.contains(scheduler),
            "the Analytics render root must schedule nothing: found {scheduler}"
        );
    }
    assert!(
        render.contains("self.busy_overlay_due()"),
        "the render root must read the overlay through busy_overlay_due — otherwise the \
         predicate below is dead code and its guards hold vacuously"
    );

    // A context-free predicate cannot acquire the scheduling APIs used by this view.
    assert!(
        src.contains("fn busy_overlay_due(&self) -> bool"),
        "busy_overlay_due must stay a pure predicate — a `&mut Context` parameter is how \
         scheduling gets back into the render path"
    );

    // A batch can contain overlapping reads but needs only one delayed repaint.
    let started = braced_body(&src, "fn op_started(");
    assert!(
        started.contains("if self.busy_since.is_none() {")
            && started.contains("BUSY_OVERLAY_DELAY")
            && started.matches("cx.spawn(").count() == 1,
        "op_started must arm its one BUSY_OVERLAY_DELAY repaint in a single place, on the \
         busy_since None -> Some transition"
    );
    // A detached timer may wake during a later batch, which must observe its own delay.
    assert!(
        started.contains("this.busy_since == Some(opened_at)"),
        "the delayed repaint must fire only for the batch that armed it"
    );
}

/// Calendar cells must keep their self-highlight in identified element state.
///
/// Adding a calendar hover field or callback couples a local border to `AnalyticsView` state and
/// lets that state outlive the series that produced the cell. An identified `.hover()` style
/// scopes the state to the cell; the id is required for GPUI to persist it between frames. This
/// does not claim to eliminate hover-triggered view repaints: GPUI still notifies the owning view
/// when element hover state changes.
///
/// Summary chart hover remains view state because it positions a sibling popup outside the
/// hovered element. This guard is limited to calendar cells that highlight themselves.
#[test]
fn calendar_hover_is_element_state_not_view_state() {
    // Include the window root, where a shared calendar hover field would be declared.
    for rel in [
        "analytics/mod.rs",
        "analytics/calendar/mod.rs",
        "analytics/calendar/day.rs",
        "analytics/calendar/month.rs",
    ] {
        assert!(
            !read_src(rel).contains("cal_hover"),
            "{rel} must not carry calendar hover state on the view"
        );
    }

    // Scan the directory so row-level callbacks and additional calendar grids cannot bypass the
    // contract.
    let mut calendar = Vec::new();
    rust_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("analytics")
            .join("calendar"),
        &mut calendar,
    );
    for path in calendar {
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
            .replace("\r\n", "\n");
        for banned in [".on_hover(", ".on_mouse_move("] {
            assert!(
                !text.contains(banned),
                "{} must not drive calendar highlighting from view state: found {banned}",
                path.display()
            );
        }
    }

    let day = read_src("analytics/calendar/day.rs");
    let hour = braced_body(&day, "fn hour_cell(");
    // `hour_cell` has no click handler to require an id independently of hover state.
    assert!(
        hour.contains(".id((\"hc\",") && hour.contains(".hover("),
        "hour_cell must highlight through a .hover() style on an identified element"
    );

    let month = read_src("analytics/calendar/month.rs");
    let cell = braced_body(&month, "fn cal_cell(");
    assert_eq!(
        cell.matches(".hover(").count(),
        1,
        "cal_cell must carry exactly one hover style"
    );
    // Date-only cards navigate but show no figures, so they must not advertise a readable value.
    let anchor = cell
        .find(".id((\"mc\",")
        .expect("cal_cell must identify its cell element");
    let chain = &cell[anchor..];
    let chain = chain.split_once(';').map_or(chain, |(head, _)| head);
    assert!(
        !chain.contains(".hover("),
        "cal_cell must attach its highlight after the date_only gate, not to the shared chain"
    );
}
