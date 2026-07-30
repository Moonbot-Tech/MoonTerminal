//! The Analytics window: its reopen state, refresh throttling, busy overlay, calendar, and
//! the render root that must schedule no repaints of its own.

use super::support::*;

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
