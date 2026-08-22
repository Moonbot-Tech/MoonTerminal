//! The Analytics window: its reopen state, refresh throttling, busy overlay, calendar, and
//! the render root that must schedule no repaints of its own.

use super::support::*;

/// Every retained view that caches civil-time presentation must observe the one shared display
/// zone revision published by the header clock.
///
/// Breakage: removing the Report observer leaves its date fields in the old city after a header
/// clock change; the same edit in Analytics, Charts, Strategies, News, Alerts, or Log leaves that
/// surface stale until an unrelated data event happens.

#[test]
fn selected_display_zone_reaches_every_cached_time_surface() {
    let backend = read_src("backend/mod.rs");
    let setter = code_only(braced_body(
        &backend,
        "pub(crate) fn set_header_clock_zone(",
    ));
    assert!(
        setter.contains("crate::chartdx::axes::set_display_zone(")
            && setter.contains("self.display_time_revision.update(cx, |_, cx| cx.notify())"),
        "the clock setter must update chart formatting and publish the shared zone revision"
    );

    for (path, label) in [
        ("panels/report/state.rs", "Report"),
        ("analytics/mod.rs", "Analytics"),
        ("strategies/state.rs", "Strategies"),
        ("panels/news/mod.rs", "News"),
        ("panels/alerts/mod.rs", "Alerts"),
        ("analytics/profit_monitor/mod.rs", "Profit Monitor"),
        ("panels/core_status/mod.rs", "Core Status"),
        ("panels/log/mod.rs", "Log"),
    ] {
        let source = code_only(&read_src(path));
        assert!(
            source.contains("cx.observe(&display_time_revision"),
            "{label} must repaint or reload when the selected display zone changes"
        );
    }

    let report = code_only(&read_src("panels/report/state.rs"));
    let report_observer = braced_body(
        &report,
        "cx.observe(&display_time_revision, |this, _revision, cx|",
    );
    assert!(
        report_observer.contains("this.data = LoadState::default()"),
        "a zone change must discard preset rows selected under the old civil bounds"
    );

    let charts = code_only(&read_src("panels/chart/mod.rs"));
    assert_eq!(
        charts.matches("cx.observe(&display_time_revision").count(),
        2,
        "both chart constructors must invalidate cached time labels"
    );

    let trade_log = code_only(&read_src("panels/report/trade_log.rs"));
    assert!(
        trade_log.contains("cx.observe(")
            && trade_log.contains("&display_time_revision")
            && trade_log.contains("view::rezone_lines(lines, zone)"),
        "an open Report trade-log dialog must rebuild its cached clocks after a zone change"
    );

    let profit_monitor = code_only(&read_module("analytics/profit_monitor"));
    assert_eq!(
        profit_monitor
            .matches("cx.observe(&display_time_revision")
            .count(),
        2,
        "the cached Profit Monitor clock and its body must both update immediately on a zone change"
    );
}

/// The Profit Monitor must keep large core sets scrollable, fixed around one virtualized body,
/// while every visible heading remains clickable and numeric values stay on one line.
///
/// Breakage: replacing `MoonVirtualList` with eager children or dropping its retained handle makes
/// a 52-core window impossible to traverse reliably; moving header/footer into the list scrolls
/// context away; removing the numeric-cell nowrap lets a large profit split across two rows;
/// leaving any Trades output unconditional or routing it from `layout.win_rate` clips or hides the
/// count at the wrong boundary instead of preserving the 310px Name-plus-Profit tier.
#[test]
fn profit_monitor_table_keeps_large_core_sets_scrollable_and_single_line() {
    let source = read_module("analytics/profit_monitor");
    let state = code_only(braced_body(&source, "pub(crate) struct ProfitMonitorView"));
    let construction = code_only(braced_body(
        &source,
        "fn new(backend: Entity<Backend>, window:",
    ));
    let table = code_only(braced_body(&source, "fn table("));
    let monitor_body = code_only(braced_body(&source, "fn body("));
    let header = code_only(braced_body(&source, "fn table_header("));
    let row = code_only(braced_body(&source, "fn table_row("));
    let split = code_only(braced_body(&source, "fn split_body("));
    let numeric = code_only(braced_body(&source, "fn numeric_cell("));

    assert!(
        state.contains("scroll: MoonVirtualListScrollHandle")
            && construction.contains("scroll: MoonVirtualListScrollHandle::new()"),
        "the monitor view must retain one virtual-list scroll handle across renders"
    );
    assert!(
        table.contains("MoonVirtualList::new(")
            && table.contains(".track_scroll(scroll)")
            && table.contains(".scrollbar_visibility(MoonScrollbarVisibility::Always)")
            && table.contains(".child(div().flex_1().min_h_0().w_full().child(body))"),
        "the bounded table viewport must use the retained virtual list and an exposed scrollbar"
    );
    let header_at = table
        .find(".child(header)")
        .expect("the fixed header must be outside the virtual list");
    let body_at = table
        .find(".child(div().flex_1().min_h_0().w_full().child(body))")
        .expect("the virtual body must occupy the bounded middle slot");
    let footer_at = table
        .find(".child(footer)")
        .expect("the fixed total footer must be outside the virtual list");
    assert!(
        header_at < body_at && body_at < footer_at,
        "the header and total must stay fixed around the scrolling rows"
    );
    assert!(
        header.contains(".cursor_pointer()")
            && header.contains("this.toggle_sort(column, cx)")
            && header.contains("MonitorSortColumn::Name")
            && header.contains("MonitorSortColumn::Profit")
            && header.contains("MonitorSortColumn::Trades")
            && header.contains("MonitorSortColumn::WinRate")
            && header.contains("MonitorSortColumn::AverageOrder"),
        "every responsive header must route clicks through the shared persisted sort action"
    );
    assert!(
        numeric.contains(".flex_none()")
            && numeric.contains(".overflow_hidden()")
            && numeric.contains(".whitespace_nowrap()")
            && numeric.contains(".text_ellipsis()"),
        "fixed numeric cells must never wrap and alter the virtual row height"
    );
    assert!(
        header.contains(".when(layout.trades")
            && row.contains(".when(show_trades")
            && table.contains("let show_trades = layout.trades;")
            && table.matches("show_trades,").count() == 2
            && split.contains(".when(show_trades")
            && monitor_body.contains("ProfitLoadState::Split(totals) => split_body(")
            && monitor_body
                .contains("MonitorLayout::for_width(width, design::ui_value(cx, 1.0)).trades",),
        "the Trades heading, body rows, total footer, and split-currency count must share one responsive decision"
    );
}

/// The Profit Monitor clock must share the terminal's selected city without making the entire
/// table repaint every second.
///
/// Breakage: moving `SECOND_MS` into `ProfitMonitorView` looks simpler but rebuilds, regroups, and
/// sorts every visible row once per second. Constructing another clock inside `controls` leaks
/// timers across rerenders instead of retaining one child entity. Making `MonitorClockView::render`
/// always call `header_clock` keeps seconds at narrow widths and clips the control row. Forking the
/// compact clock's picker lets its city diverge from the full header. Removing `size_full()` from
/// the inner `v_flex` in `ProfitMonitorBodyView::render` sizes its root to the header and footer, so
/// the virtual viewport collapses and hides every grouped row while the nonzero total remains
/// visible.
#[test]
fn profit_monitor_clock_ticks_in_one_retained_child_view() {
    let source = read_module("analytics/profit_monitor");
    let shared_clock = read_src("chrome/clock.rs");
    let state = code_only(braced_body(&source, "pub(crate) struct ProfitMonitorView"));
    let construction = code_only(braced_body(
        &source,
        "fn new(backend: Entity<Backend>, window:",
    ));
    let clock_construction =
        code_only(braced_body(&source, "fn new(backend: Entity<Backend>, cx:"));
    let clock_render = code_only(braced_body(&source, "impl Render for MonitorClockView"));
    let full_clock = code_only(braced_body(&shared_clock, "pub(crate) fn header_clock("));
    let compact_clock = code_only(braced_body(
        &shared_clock,
        "pub(crate) fn compact_header_clock(",
    ));
    let shared_clock_render = code_only(braced_body(&shared_clock, "fn render_header_clock("));
    let controls = code_only(braced_body(&source, "fn controls("));
    let body_render = code_only(braced_body(
        &source,
        "impl Render for ProfitMonitorBodyView",
    ));
    let parent_render = code_only(braced_body(&source, "impl Render for ProfitMonitorView"));
    let invalidate = code_only(braced_body(&source, "fn invalidate_content("));

    assert!(
        state.contains("clock: Entity<MonitorClockView>")
            && construction
                .matches("cx.new(|cx| MonitorClockView::new(backend.clone(), cx))")
                .count()
                == 1
            && controls.contains(".child(self.clock.clone())"),
        "the monitor must retain one child clock instead of constructing it while rendering"
    );
    assert!(
        clock_construction.contains("duration_until_wall_clock_boundary(")
            && clock_construction.contains("SECOND_MS")
            && clock_construction.contains("this.update(cx, |_this, cx| cx.notify())")
            && source.matches("SECOND_MS").count() == 2,
        "only MonitorClockView may own the wall-clock-aligned second repaint"
    );
    assert!(
        clock_render
            .contains("MonitorLayout::for_width(window_width(window), design::ui_value(cx, 1.0))",)
            && clock_render.contains(".clock_seconds")
            && clock_render
                .contains("crate::chrome::clock::header_clock(&self.backend, palette, cx)")
            && clock_render
                .contains("crate::chrome::clock::compact_header_clock(&self.backend, palette, cx)"),
        "the retained monitor clock must select full or compact shared presentation from width"
    );
    assert!(
        full_clock.contains("render_header_clock(backend, p, ClockPrecision::Seconds, cx)")
            && compact_clock
                .contains("render_header_clock(backend, p, ClockPrecision::Minutes, cx)")
            && shared_clock_render.contains("let selected = selected_zone(backend, cx);")
            && shared_clock_render.contains("MoonPopover::new(\"header-clock-popover\")"),
        "both clock precisions must share one selected-zone renderer and picker"
    );
    assert!(
        state.contains("content: Entity<ProfitMonitorBodyView>")
            && construction.contains("owner: content_owner")
            && body_render.contains(".read(cx)")
            && parent_render.contains("AnyView::from(self.content.clone())")
            && parent_render.contains(".cached(")
            && parent_render.contains("StyleRefinement::default()")
            && parent_render.contains(".flex_1()")
            && parent_render.contains(".min_h(px(0.0))")
            && parent_render.contains(".w_full()")
            && !parent_render.contains("div().flex_1().min_h_0().w_full().child("),
        "the cached body must remain the direct flex child that receives the parent's remaining bounds"
    );
    assert!(
        body_render.contains("let body = owner")
            && body_render.contains(".read(cx)")
            && body_render.contains("v_flex()")
            && body_render.contains(".size_full()")
            && body_render.contains(".min_h_0()")
            && body_render.contains(".child(body)")
            && body_render.matches("v_flex()").count() == 1,
        "the cached view's inner vertical root must fill its allocated bounds before laying out the elastic table"
    );
    assert!(
        invalidate.contains("self.content.update(cx, |_content, cx| cx.notify())"),
        "body-state writers need a dedicated cached-child invalidation path"
    );
    for writer in [
        "fn sync_context(",
        "fn reload(",
        "fn set_group(",
        "fn toggle_sort(",
    ] {
        assert!(
            code_only(braced_body(&source, writer)).contains("invalidate_content(cx)"),
            "{writer} must invalidate the cached body after changing one of its inputs"
        );
    }
}

/// Profit Monitor values must carry their rounded sign into the semantic palette, while the fixed
/// total row is visibly stronger than ordinary data rows.
///
/// Breakage: recolouring from the raw value turns a displayed zero red; dropping the footer height,
/// weight, background, or accent border makes the grand total indistinguishable from the final
/// data row.
#[test]
fn profit_monitor_profit_tones_and_total_emphasis_stay_wired() {
    let source = read_module("analytics/profit_monitor");
    let table = code_only(braced_body(&source, "fn table("));
    let row = code_only(braced_body(&source, "fn table_row("));
    let split = code_only(braced_body(&source, "fn split_body("));

    assert!(
        table.matches("format_profit(").count() == 2
            && table.contains("profit_sign,")
            && table.contains("total_profit_sign,"),
        "body and total rows must pass the sign returned beside their formatted profit"
    );
    assert!(
        row.contains("profit_sign.pick(")
            && row.contains("design::positive_color(palette)")
            && row.contains("design::danger_color(palette)")
            && row.contains("palette.text")
            && row.contains(".text_color(moon(profit_color))"),
        "the profit cell must use the theme's semantic positive, danger, and neutral tones"
    );
    assert!(
        split.contains("design::positive_color(palette)")
            && split.contains("design::danger_color(palette)")
            && split.contains("palette.text"),
        "split-currency profit chips must use the same theme-safe semantic tones"
    );
    // Sliced at the footer's own statement, not searched across the whole function: a group
    // subtotal drawn inside the item builder above shares two of these markers, and a whole-body
    // search would keep passing after the footer itself lost them.
    let footer_at = table
        .find("let footer = table_row(")
        .expect("the fixed total footer must be built in the table body");
    let footer = &table[footer_at..];
    for marker in [
        ".h(design::fit_h_px(cx, 42.0, 14.0, 10.0))",
        ".bg(moon(palette.table_head))",
        ".text_size(design::t_title(cx))",
        ".font_weight(FontWeight::SEMIBOLD)",
        ".border_t(px(2.0))",
        ".border_color(moon_alpha(palette.amber, 0.7))",
    ] {
        assert!(footer.contains(marker), "the total row lost `{marker}`");
    }
}

/// Profit Monitor sections must read as one hierarchy without weakening the fixed grand total.
///
/// Breakage: changing `table.rs:table` to assign `RowRole::Plain` to a section member removes its
/// inset and continuation rail, so core names align flat with the group heading and users can no
/// longer scan where one saved group begins and ends.
///
/// Mutation: replace the `RowRole::SectionMember` assignment in `table` with `RowRole::Plain`; the
/// member-role assertion below must fail while the other hierarchy roles remain present.
#[test]
fn profit_monitor_group_hierarchy_stays_visually_distinct() {
    let source = read_module("analytics/profit_monitor");
    let table = code_only(braced_body(&source, "fn table("));
    let row = code_only(braced_body(&source, "fn table_row("));
    let header = code_only(braced_body(&source, "fn section_header("));

    assert_eq!(
        table.matches("RowRole::SectionMember").count(),
        1,
        "every row under a visible group heading must receive the nested member role"
    );
    assert!(
        table.contains("RowRole::SectionSubtotal")
            && table.contains("RowRole::Plain")
            && table.contains("profit_monitor.grand_total"),
        "section subtotals, flat rows, and the fixed grand total need distinct presentation roles"
    );
    assert!(
        header.contains(".border_l(px(2.0))")
            && header.contains("palette.border_soft")
            && header.contains("design::t_body_lg(cx)")
            && header.contains("text_tooltip(head.name.clone())"),
        "a group heading needs a quiet boundary rail, stronger label, and its full-name tooltip"
    );
    assert!(
        row.contains("role != RowRole::Plain")
            && row.contains("SECTION_MEMBER_INDENT")
            && row.contains("role == RowRole::SectionSubtotal")
            && row.contains(".border_t(px(1.0))"),
        "members must continue the inset rail and a subtotal must visibly close the group"
    );
    assert!(
        table.contains("let subtotal_tooltip = subtotal.then(|| name.clone())")
            && table.contains("text_tooltip(label)"),
        "a truncated subtotal must retain its complete localized label in a tooltip"
    );
}

/// Profit Monitor control types and every persisted choice must stay coupled to their restore and
/// writer paths instead of becoming session-only view state.
///
/// Breakage: replacing the period dropdown with another segmented strip recreates the crowded
/// header; forcing the `layout.inline_controls` branch keeps every control on one clipped row below
/// 460px; restoring `open` to a literal unscaled 460px minimum blocks the supported narrow window,
/// while unscaled 390px clips it above 100% UI scale; deleting any layout assignment or constructor
/// restore silently resets that choice after restart even though the layout serializer's isolated
/// round-trip remains green.
#[test]
fn profit_monitor_controls_and_all_choice_persistence_stay_wired() {
    let source = read_module("analytics/profit_monitor");
    let construction = code_only(braced_body(
        &source,
        "fn new(backend: Entity<Backend>, window:",
    ));
    let controls = code_only(braced_body(&source, "fn controls("));
    let period_dropdown = code_only(braced_body(&source, "fn period_dropdown("));
    let open = code_only(braced_body(&source, "fn open_window("));
    let set_period = code_only(braced_body(&source, "fn set_period("));
    let set_group = code_only(braced_body(&source, "fn set_group("));
    let toggle_sort = code_only(braced_body(&source, "fn toggle_sort("));

    assert!(
        period_dropdown.contains("MoonDropdown::new(\"profit-monitor-period\")")
            && controls
                .matches("period_dropdown(self.period, cx.entity())")
                .count()
                == 1
            && controls.contains("let groups = [GroupMode::Core, GroupMode::Exchange];")
            && controls
                .matches("MoonSegmentedControl::new(\"profit-monitor-groups\")")
                .count()
                == 1,
        "period must use one dropdown while Core/Exchange remain exactly two segmented buttons"
    );
    assert!(
        controls
            .contains("let layout = MonitorLayout::for_width(width, design::ui_value(cx, 1.0));",)
            && controls.contains("if layout.inline_controls")
            && controls.contains("v_flex()")
            && controls.contains(".child(h_flex().w_full().justify_center().child(group_control))"),
        "narrow controls must move complete period/clock and grouping units onto two rows"
    );
    assert!(
        open.contains("Some(size(design::ui_px(cx, MIN_WINDOW_WIDTH), px(320.0)))"),
        "the OS window minimum must scale the responsive-width constant with its rendered geometry"
    );
    for key in [
        "profit_monitor_period",
        "profit_monitor_group",
        "profit_monitor_sort",
    ] {
        assert!(
            construction.contains(key),
            "the monitor constructor must restore {key}"
        );
    }
    for (name, body, key) in [
        ("period", set_period, "profit_monitor_period"),
        ("group", set_group, "profit_monitor_group"),
        ("sort", toggle_sort, "profit_monitor_sort"),
    ] {
        assert!(
            body.contains(key) && body.contains("backend.layout_dirty = true"),
            "the {name} choice must update {key} and mark the layout dirty"
        );
    }
}

/// The per-core Summary ranking must stay virtualized in both modes and use MoonUI's progress
/// primitive for magnitude bars.
///
/// Breakage: replacing either `MoonVirtualList` with eager rows lets the elastic card clip its
/// leaders at minimum window height; dropping `MoonScrollbarVisibility::Always` makes overflow
/// undiscoverable until the pointer happens to cross it; hand-building the progress track forks
/// MoonUI's scaling and theme behavior.
#[test]
fn per_core_summary_rankings_stay_virtualized_and_moonui_first() {
    let charts = read_src("analytics/summary/charts.rs");
    let overview = code_only(braced_body(&charts, "fn core_rank_overview("));
    let all = code_only(braced_body(&charts, "fn core_rank_all("));
    let row = code_only(braced_body(&charts, "fn core_rank_row("));

    for (name, body, id) in [
        ("overview", overview, "an-core-rank-overview"),
        ("all", all, "an-core-rank-all"),
    ] {
        assert!(
            body.contains(&format!("MoonVirtualList::new(\"{id}\""))
                && body.contains(".scrollbar_visibility(MoonScrollbarVisibility::Always)"),
            "the {name} ranking must use its own virtual list with an always-visible scrollbar"
        );
        assert_eq!(
            body.matches(".pr(scrollbar_gutter)").count(),
            1,
            "the {name} ranking must reserve the overlay gutter once at its outer right edge"
        );
    }
    assert!(
        row.contains("MoonProgress::new(")
            && row.contains(".value(row.magnitude_pct)")
            && row.contains(".color(super::sign_color(p, row.total))"),
        "each ranking row must delegate normalized, sign-aware bars to MoonProgress"
    );
}

/// The Summary triptych must share one card height, while Insights mirrors the neighbouring trade
/// tables and keeps the complete conclusion behind each populated row.
///
/// Breakage: restoring `items_start` makes the Insights border shorter than its two neighbours;
/// changing `analytics/summary/mod.rs:insights_card` back to caption-sized semibold body cells makes
/// the third card visibly diverge from Top/Worst; dropping a slot or tooltip hides period facts.
#[test]
fn summary_triptych_and_insight_rows_keep_their_visual_contract() {
    let summary = read_src("analytics/summary/mod.rs");
    let tab = code_only(braced_body(&summary, "pub(super) fn summary_tab("));
    let rows = code_only(braced_body(&summary, "fn insight_rows("));
    let card = code_only(braced_body(&summary, "fn insights_card("));
    let header = chain_between(
        &card,
        "let mut list = v_flex().w_full().gap_0().child(",
        ");\n    for (ix, row)",
        "Insights table header",
    );
    let body = chain_between(
        &card,
        "let mut element = h_flex()",
        "if let Some(tooltip)",
        "Insights body row",
    );
    let shell = chain_between(
        &card,
        "v_flex()\n        .flex_1()",
        ".child(list)",
        "Insights card shell",
    );

    assert!(
        tab.contains(
            ".items_stretch()\n                    .child(top_card(\n                        t!(\"analytics.best_trades\")"
        ) && tab.contains(".child(insights_card(&data, p, cx))"),
        "the trade/insight triptych must stretch all three cards to one height"
    );
    assert!(
        rows.contains("[strategy, contribution, risk, quality, hour]")
            && rows.contains("main: \"—\".to_string()")
            && rows.contains("metric: String::new()")
            && rows.contains("metric_color: p.text_muted")
            && rows.contains("tooltip: None"),
        "Insights must retain five stable slots with fully neutral placeholders for missing facts"
    );
    for tooltip_key in [
        "analytics.ins.best_strategy",
        "analytics.ins.top_coin",
        "analytics.ins.worst_coin",
        "\"analytics.ins.pf\",",
        "analytics.ins.best_hour",
    ] {
        assert!(
            rows.contains(tooltip_key),
            "the populated Insight row must retain the full {tooltip_key:?} tooltip"
        );
    }
    assert!(
        header.contains(".h(design::fit_h_px(cx, 22.0, 12.0, 5.0))")
            && header.contains(".px(design::ui_px(cx, 8.0))")
            && header.contains(".gap(design::ui_px(cx, 8.0))")
            && header.contains(".text_size(design::t_caption(cx))")
            && header.contains(".bg(moon(p.table_head))")
            && header.contains(".max_w(label_w)")
            && header.contains(".max_w(metric_w)")
            && header.contains(".text_right()")
            && header.contains("analytics.ins.col.insight")
            && header.contains("analytics.ins.col.detail")
            && header.contains("analytics.ins.col.result"),
        "Insights must use a trade-table header with columns aligned to its body"
    );
    assert!(
        body.contains(".h(row_h)")
            && body.contains(".px(design::ui_px(cx, 8.0))")
            && body.contains(".gap(design::ui_px(cx, 8.0))")
            && body.contains(".bg(moon(p.table_body))")
            && body.contains(".border_t_1()")
            && body.contains(".max_w(label_w)")
            && body.contains(".max_w(metric_w)")
            && body.matches(".truncate()").count() == 3
            && body.contains(".text_right()")
            && body.contains(".text_color(moon(row.metric_color))")
            && !body.contains(".text_size(design::t_caption(cx))")
            && !body.contains(".font_weight(FontWeight::SEMIBOLD)"),
        "Insights body rows must share trade-table geometry and normal body typography"
    );
    assert!(
        shell.contains(".rounded(design::ui_px(cx, 8.0))")
            && shell.contains(".bg(moon(p.panel))")
            && shell.contains(".border_1()")
            && shell.contains(".overflow_hidden()")
            && shell.contains(".px(design::ui_px(cx, 12.0))")
            && shell.contains(".py(design::ui_px(cx, 8.0))")
            && shell.contains(".text_size(design::t_title(cx))")
            && shell.contains(".font_weight(FontWeight::SEMIBOLD)")
            && !shell.contains(".gap(")
            && !shell.contains("analytics.insights_sub"),
        "Insights must share the trade cards' frame and one-line title instead of an inset layout"
    );
    assert!(
        card.contains("text_tooltip(tooltip)") && !card.contains(".flex_none()"),
        "Insight truncation must stay safe and expose each complete conclusion on hover"
    );
}

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

/// A Report column added later must be migrated into the PER-CONTEXT visible sets, not only into
/// the `app_meta` seed.
///
/// The constructor reads a per-context set wherever a `:dock` entry exists, while detached mode
/// applies the corresponding `:win` set through `apply_ctx_columns`;
/// so a migration that touches `app_meta` alone reaches exactly the users who never arranged their
/// columns — and misses everyone who did, the person who asked for the column first among them.
///
/// Its completion marker must live in the window layout beside the sets it rewrites — see
/// `WindowLayout::report_columns_migration` for why the two stores may not be split.
///
/// Breakage: deleting the `migrate_ctx_visible` call as redundant with `db::load_visible`, or
/// moving its counter back into `app_meta`, where the two stores can disagree.
#[test]
fn report_columns_added_later_migrate_the_per_context_sets() {
    let state = read_src("panels/report/state.rs");
    let construction = braced_body(&state, "pub(crate) fn new_with_scope(");
    let migration = construction
        .find("migrate_ctx_visible(")
        .expect("panel construction must migrate the per-context column sets");
    let applied = construction
        .find("crate::persistence::table_persist::visible(")
        .expect("panel construction must read the migrated per-context column set");
    assert!(
        migration < applied,
        "the migration must run before the set it migrates is read"
    );
    let guard = braced_body(&state, "fn migrate_ctx_visible(");
    assert!(
        guard.contains("layout.report_columns_migration")
            && !guard.contains("columns_migration(conn"),
        "the one-shot marker must live in the same document as the sets it guards"
    );
    let marked = guard
        .find("layout.report_columns_migration = Some(")
        .expect("the migration must record its own completion");
    assert!(
        guard
            .find("migrate_visible_sets(")
            .is_some_and(|at| at < marked),
        "the sets must be rewritten before the migration marks itself done"
    );
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
        .find("let strategy_filter = design::chrome_section(cx)")
        .expect("Report controls must retain the exact strategy section");
    let mask = render
        .find("let strategy_mask = self")
        .expect("Report controls must retain the independent Auto mask section");
    let wrapping = render
        .find("let filters = h_flex()")
        .expect("Report controls must retain narrow-width wrapping");
    let exact_attached = render
        .find(".child(separated(strategy_filter))")
        .expect("the exact strategy section must enter the wrapping filter row");
    let mask_attached = render
        .find(".children(strategy_mask.map(separated))")
        .expect("the Auto mask must enter as its own wrapping section");
    assert!(
        strategy < mask
            && mask < wrapping
            && wrapping < exact_attached
            && exact_attached < mask_attached
            && !render.contains(".overflow_x_scroll()"),
        "the exact selector and Auto mask must wrap independently without horizontal scroll"
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
        filter
            .contains("rows: super::row_scope_for(self.closed_only, self.show_open, date_to, now)")
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

/// Auto Report must keep its strategy-name mask workspace-safe and its wrapping toolbar semantic.
///
/// Plausible breakages: hiding the retained mask in Overview or applying it in Classic; dropping it
/// from persistence or stale-query identity; restoring the shared fixed core-trigger width; using
/// DB history before the live group name; or placing free dividers between wrapping controls so a
/// separator can wrap alone. The binary view has no importable library target, so this contract
/// pins that otherwise-unreachable GPUI wiring while unit tests cover the pure name/filter rules.
#[test]
fn auto_report_mask_and_grouped_toolbar_stay_scope_safe() {
    let render = read_src("panels/report/render.rs");
    let report_mod = read_src("panels/report/mod.rs");
    let controls = read_src("panels/report/controls.rs");
    let query = read_src("panels/report/query.rs");
    let state = read_src("panels/report/state.rs");
    let actions = read_src("panels/report/actions.rs");

    let mask_field = braced_body(&controls, "pub(super) fn strategy_name_mask_field(");
    for needle in [
        "super::strategy_name_mask_enabled(",
        "MoonInput::new(\"rep-strategy-mask\")",
        ".state(&self.strategy_name_mask_input)",
        "report.filter.strategy_mask_tip",
    ] {
        assert!(
            mask_field.contains(needle),
            "the separate Auto-workspace mask field must retain {needle:?}"
        );
    }

    let core_combo = braced_body(&controls, "pub(super) fn core_combo(");
    for needle in [
        "backend.group_cores(&self.group)",
        "selected_auto_core_name(core, &live_cores, &cores)",
        ".fit_trigger_width(",
        "AUTO_CORE_TRIGGER_MAX_W",
        "host.tooltip(crate::panels::common::text_tooltip(label))",
    ] {
        assert!(
            core_combo.contains(needle),
            "the pinned Auto core label must retain {needle:?}"
        );
    }

    let filter = braced_body(&query, "pub(super) fn filter(");
    assert!(
        filter.contains("super::strategy_name_mask_enabled(")
            && filter.contains("strategy_name_mask,")
            && filter.contains("self.strategy_name_mask.trim().to_string()"),
        "both Auto scopes, but no Classic host, may copy the retained mask into ReportFilter"
    );
    let eligibility = braced_body(&report_mod, "fn strategy_name_mask_enabled(");
    assert!(
        eligibility.contains("EffectiveCoreScope::is_workspace_owned"),
        "one shared workspace-owned predicate must keep Auto Overview and AutoCore aligned"
    );
    let catalog = braced_body(&query, "fn strategy_catalog_scope(");
    assert!(
        catalog.contains("scope.strategies = None;")
            && catalog.contains("scope.strategy_name_mask.clear();"),
        "the exact strategy catalog must remain independent of both strategy filters"
    );

    let body = braced_body(&render, "fn render(");
    for needle in [
        ".flex_wrap()",
        "let separated = |section: Div|",
        ".child(design::chrome_divider(cx, p))",
        "design::chrome_section(cx)",
        ".strategy_name_mask_field(cx)",
        ".children(strategy_mask.map(separated))",
        ".children(date_filters.into_iter().flatten().map(separated))",
        ".child(separated(actions).ml_auto())",
    ] {
        assert!(
            body.contains(needle),
            "the wrapping semantic toolbar must retain {needle:?}"
        );
    }
    assert!(
        body.matches("design::chrome_section(cx)").count() >= 7
            && !body.contains(".overflow_x_scroll()"),
        "Report filter concepts must stay grouped and wrap without horizontal scrolling"
    );

    let construction = braced_body(&state, "pub(crate) fn new_with_scope(");
    for needle in [
        "report.filter.strategy_mask_ph",
        "&strategy_name_mask_input",
        "panel.strategy_name_mask = value;",
        "panel.persist_filters(None, cx);",
        "panel.request_requery(cx);",
    ] {
        assert!(
            construction.contains(needle),
            "the retained mask input must preserve {needle:?}"
        );
    }
    let restore = braced_body(&state, "pub(super) fn restore_persisted_filters(");
    assert!(
        restore.contains("self.strategy_name_mask_input")
            && restore.contains("input.sync_value(mask, input_cx)"),
        "host restoration must synchronize the retained MoonUI input before querying"
    );
    let persist = braced_body(&actions, "pub(super) fn persist_filters(");
    assert!(
        persist.contains("next_prefs_for_period_pick(")
            && persist.contains("&super::state::ReportFilterSet {")
            && persist.contains("show_open: self.show_open,"),
        "every Report filter write must pass the complete named filter set, including active positions"
    );
    let persist_helper = braced_body(&report_mod, "pub(super) fn next_prefs_for_period_pick(");
    assert!(
        persist_helper.contains("prefs.strategy_name_mask = Some(live.strategy_name_mask.clone())")
            && persist_helper.contains("prefs.show_open = Some(live.show_open);"),
        "the persistence composer must write both the Auto strategy-name mask and active-positions switch"
    );
}

/// Analytics tabs size from their title while the complete core/filter group wraps together.
///
/// This binary crate exposes no importable GPUI view. The source contract pins the content-sized
/// tabs, the atomic wrapping filter group, and the conditional pre-dropdown clear button whose
/// width must participate in that group's responsive floor.
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
    assert!(
        toolbar.contains("crate::controls::CoreAllRowMode::ImplicitOnly")
            && body.contains(
                "let clear_core_filter = (!workspace_pinned && !self.sel_cores.is_empty()).then(||"
            )
            && body.contains(".on_click(cx.listener(|this, _, _, cx| this.toggle_core(None, cx)))"),
        "Classic Analytics must keep All exclusive while Auto hides retained-filter mutation controls"
    );
    for needle in [
        "let clear_core_filter_w = if workspace_pinned || self.sel_cores.is_empty()",
        "design::glyph_btn_w(cx) + design::ui_value(cx, TOOLBAR_GAP)",
        "let filters_min_w = clear_core_filter_w",
    ] {
        assert!(
            body.contains(needle),
            "the conditional core-clear button must participate in the responsive floor via {needle:?}"
        );
    }
    let core_combo_body = braced_body(&toolbar, "fn core_combo(");
    assert!(
        core_combo_body.contains("let filter_pin = self.core_filter_pin();")
            && core_combo_body.contains("let workspace_pinned = filter_pin.is_some();")
            && core_combo_body.contains(".disabled(workspace_pinned)"),
        "the Auto-owned core selector must display effective scope without mutating retained Classic state"
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
    let clear_filter = selectors
        .find(".children(clear_core_filter)")
        .expect("the selector row must contain the conditional clear button");
    let core_filter = selectors
        .find(".child(self.core_combo(cx))")
        .expect("the selector row must contain the core dropdown");
    assert!(
        clear_filter < core_filter,
        "the conditional clear button must render immediately before the core dropdown"
    );
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
    let startup = read_startup();
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
    let cores_selected = braced_body(&analytics, "fn cores_selected(");
    let read_core_ids = braced_body(&analytics, "fn read_core_ids(");
    let filter_ids = braced_body(&toolbar, "pub(super) fn analytics_core_filter_ids(");
    assert!(
        cores_selected.contains("&self.sel_cores")
            && cores_selected.contains("self.read_core_ids()")
            && read_core_ids.contains("scope.core_ids.as_slice()")
            && filter_ids.contains("Some([]) => vec![0]")
            && filter_ids.contains("Some(cores) => cores.to_vec()")
            && filter_ids.contains("None => selected.iter().copied().collect()"),
        "Analytics queries must preserve retained Classic selection while using concrete Auto ids and an explicit empty-scope no-match, reading the filter (not action) authority"
    );
    let workspace_observer = analytics
        .split("cx.observe(&workspace_revision")
        .nth(1)
        .and_then(|tail| tail.split(".detach();").next())
        .expect("Analytics must observe the shared workspace revision");
    assert!(
        !workspace_observer.contains("sel_cores ="),
        "workspace changes must never overwrite the process-lifetime Classic core selection"
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
/// The plausible regression is replacing `show_overlay` with `true` at any cancellable read below:
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
            &["fn reload_filter_axis_inner("][..],
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
                .split_once("self.spawn_latest_db(")
                .unwrap_or_else(|| panic!("{rel}: {signature} must start its database read"))
                .1
                .trim_start();
            let policy_args = spawn_args
                .split_once("move ||")
                .unwrap_or_else(|| panic!("{rel}: {signature} must supply its database work"))
                .0;
            assert!(
                policy_args.contains("show_overlay,"),
                "{rel}: {signature} must pass the explicit presentation policy to spawn_latest_db"
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
    let preserve_snapshot = "let preserve_snapshot =\n                    after_report && !matches!(this.strategy_data, ProfitLoadState::Loading);";
    assert!(
        reload.contains(preserve_snapshot),
        "only a settled strategy snapshot may survive an automatic report refresh"
    );
    let automatic_result = chain_between(
        reload,
        "if !preserve_snapshot || data_error.is_none() {",
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

/// Automatic Report refresh must start only from a rendered panel, without requiring OS focus.
/// Generation callbacks, timers, constructors, and in-flight completions may publish only a wake
/// edge. Wrapping the render-boundary calls in `window.is_window_active()` leaves a visible Report
/// in an unfocused window blank or stale until the user clicks it.
#[test]
fn report_refresh_stays_render_bounded_without_os_focus() {
    let query = read_src("panels/report/query.rs");
    let render = read_src("panels/report/render.rs");
    let state = read_src("panels/report/state.rs");
    let generation = braced_body(&query, "pub(super) fn requery_on_generation(");
    let schedule = braced_body(&query, "pub(super) fn schedule_requery(");
    let report_render = code_only(braced_body(&render, "fn render(&mut self, window:"));
    let query_boundary = chain_between(
        &report_render,
        "self.sync_display_zone_fields(window, cx);",
        "self.flush_strategy_select_sync(window, cx);",
        "visible Report query boundary",
    );
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
        "a hidden Report constructor must defer its initial query to panel render"
    );
    assert!(
        query_boundary.contains("self.generation_refresh.take_due()")
            && query_boundary.contains("self.schedule_requery(cx);")
            && !query_boundary.contains("window.is_window_active()"),
        "a rendered Report must consume due work and start pending queries without OS focus"
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

/// `analytics/calendar/month.rs:AnalyticsView::cal_kpi` changing either localized tooltip
/// argument to `None` must fail here, or the Costs/Funding explanation disappears on hover while
/// the card still compiles and looks unchanged.
#[test]
fn calendar_cost_and_funding_tiles_keep_localized_tooltips() {
    let month = read_src("analytics/calendar/month.rs");
    let cal_kpi = code_only(braced_body(&month, "fn cal_kpi("));
    for key in ["analytics.cal.kpi_fee_tip", "analytics.cal.kpi_funding_tip"] {
        assert!(
            cal_kpi.contains(&format!("Some(t!(\"{key}\").to_string())")),
            "Calendar Month must attach the localized {key} explanation to its KPI tile"
        );
    }

    let tile = code_only(braced_body(&month, "pub(super) fn kpi_tile("));
    assert!(
        tile.contains(".id(id)")
            && tile.contains("tile.tooltip(crate::panels::common::text_tooltip(tooltip))"),
        "the identified KPI root must delegate hover copy to the standard MoonUI tooltip adapter"
    );

    let locales = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../locales/analytics.yml"),
    )
    .expect("failed to read locales/analytics.yml")
    .replace("\r\n", "\n");
    for (key, next) in [
        ("analytics.cal.kpi_funding", "analytics.cal.funding_short:"),
        ("analytics.cal.funding_short", "analytics.cal.kpi_fee_tip:"),
        (
            "analytics.cal.kpi_fee_tip",
            "analytics.cal.kpi_funding_tip:",
        ),
        ("analytics.cal.kpi_funding_tip", "analytics.cal.fee_short:"),
    ] {
        let block = chain_between(
            &locales,
            &format!("{key}:\n"),
            next,
            "Calendar KPI locale block",
        );
        let members = block
            .lines()
            .filter(|line| line.starts_with("  "))
            .collect::<Vec<_>>();
        assert_eq!(members.len(), 3, "{key} must define exactly ru, en, and es");
        for locale in ["ru", "en", "es"] {
            assert!(
                members
                    .iter()
                    .any(|line| line.starts_with(&format!("  {locale}: "))),
                "{key} must define {locale}"
            );
        }
    }
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

/// The Report totals row must degrade by priority, and every fact it states must be one the
/// tooltip already knows about.
///
/// This is the structural half of "the tooltip repeats everything that can clip": the unit test in
/// `totals/tests.rs` can only check the facts it is given, so what makes the guarantee real is that
/// `render.rs` builds the row from `footer_facts` ALONE. A fact constructed inline beside them
/// would render without ever reaching `footer_tooltip`, and on a narrow dock that fact becomes
/// unreachable — precisely what the tooltip exists to prevent.
///
/// Breakage: "fixing" narrow layout by putting `.flex_wrap()` back on the totals row, which trades
/// a fixed-height footer for one that grows and pushes the table; or splitting the tail into two
/// shrinkable boxes, since flex shrink is proportional to base size and two siblings would erode
/// higher-priority facts alongside lower-priority ones.
#[test]
fn the_report_totals_row_degrades_by_priority_not_by_wrapping() {
    let render = read_src("panels/report/render.rs");
    let body = braced_body(&render, "fn render(");
    let totals = read_src("panels/report/totals.rs");

    for anchor in [
        "totals::footer_facts(",
        "totals::footer_tooltip(&facts)",
        // Both halves must actually reach the tree. Rendering only the head would keep every other
        // assertion here green while quietly deleting the clippable facts. The anchors start at the
        // builder calls so reflowing the surrounding block does not redden this.
        "fact_group(\"rep-totals-head\"",
        "fact_group(\"rep-totals-tail\"",
        // The right-pinned group is built the same way, so its fact also reaches the tooltip.
        "fact_group(\"rep-totals-shown\"",
        "facts.essential)",
        "facts.tail)",
        "facts.trailing)",
    ] {
        assert!(
            body.contains(anchor),
            "the totals row must be assembled through {anchor}"
        );
    }
    for inline in [
        "report.totals_n",
        "report.shown_top",
        "report.valuation_total",
        "report.unknown_quote_orders",
        "report.traded_volume",
    ] {
        assert!(
            !body.contains(inline),
            "{inline} must reach the row through footer_facts, not as an inline chip"
        );
    }
    assert_eq!(
        body.matches(".overflow_hidden()").count(),
        1,
        "exactly one clipping box: the fact tail"
    );
    assert_eq!(
        body.matches(".flex_wrap()").count(),
        1,
        "the only wrapping row left in Report's render is the filters row"
    );
    assert!(
        totals.contains("volume.section_start = true;")
            && totals.contains("\"report.traded_volume\"")
            && totals.contains("\"report.traded_volume_tip\"")
            && totals.contains("\"report.traded_volume_current\"")
            && totals.contains("\"report.traded_volume_current_tip\"")
            && body.contains(".when(fact.section_start")
            && body.contains(".border_l_1()")
            && body.contains(".border_color(rgb(p.border))"),
        "volume must originate in footer_facts and render one palette-token separator"
    );

    let locales = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../locales/report.yml"),
    )
    .expect("read Report locales")
    .replace("\r\n", "\n");
    for (key, next) in [
        ("report.traded_volume", "report.traded_volume_tip:"),
        ("report.traded_volume_tip", "report.traded_volume_current:"),
        (
            "report.traded_volume_current",
            "report.traded_volume_current_tip:",
        ),
        (
            "report.traded_volume_current_tip",
            "report.traded_volume_partial:",
        ),
        (
            "report.traded_volume_partial",
            "report.traded_volume_partial_tip:",
        ),
        (
            "report.traded_volume_partial_tip",
            "report.traded_volume_unknown_quote:",
        ),
        (
            "report.traded_volume_unknown_quote",
            "report.unknown_quote_orders:",
        ),
    ] {
        let block = chain_between(
            &locales,
            &format!("{key}:\n"),
            next,
            "Report traded-volume locale block",
        );
        for locale in ["ru", "en", "es"] {
            assert!(
                block
                    .lines()
                    .any(|line| line.starts_with(&format!("  {locale}: "))),
                "{key} must define {locale}"
            );
        }
        assert!(
            !block.contains('|'),
            "the visual separator belongs to render code, not {key} locale text"
        );
    }
    // (`.overflow_x_scroll()` is already banned across this file by
    // `report_table_uses_scrollable_preserved_widths`; repeating it here would pin nothing new.)

    // The commands are the only way to act on a selection, so they are the row's shrinkable
    // sibling: allowed to wrap internally, never to clip.
    let controls = read_src("panels/report/controls.rs");
    let actions = braced_body(&controls, "pub(super) fn selection_actions(");
    assert!(
        actions.contains(".min_w_0()") && actions.contains(".flex_wrap()"),
        "the selection commands must absorb a narrow row by wrapping"
    );
    assert!(
        !actions.contains(".ml_auto()"),
        "the fact group's zero flex basis pins the commands right; a second mechanism would fight it"
    );
}

/// The report footer must learn that valuation is stuck from the WORKER, not from row counts.
///
/// Breakage: a cleanup deleting the `valuation_status` plumbing and deriving "stuck" from
/// `coverage.valued_orders` standing still. A count cannot tell a slow backfill apart from a worker
/// retrying an unreachable provider forever, so the panel would show a frozen ratio with no
/// explanation.
///
/// Scoped to the whole `panels/report` directory rather than to `render.rs`, so moving the
/// footer's fact assembly into a sibling module does not falsify it.
#[test]
fn the_report_footer_reads_stall_from_worker_health_not_row_counts() {
    let mut sources = Vec::new();
    rust_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("panels")
            .join("report"),
        &mut sources,
    );
    let text = sources
        .iter()
        .map(|path| {
            fs::read_to_string(path)
                .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
                .replace("\r\n", "\n")
        })
        .collect::<Vec<_>>()
        .join("\n");

    for anchor in [
        // The handle owns the revision-then-snapshot read order; reading the two fields by hand
        // here is what would reintroduce the swallowed-transition race.
        "valuation.seed_status()",
        "valuation.status_if_changed(",
        "valuation_health::stall_facts(",
        "is_retrying()",
        "report.valuation_stalled",
        "report.valuation_retrying",
    ] {
        assert!(
            text.contains(anchor),
            "the report footer must render worker health through {anchor}"
        );
    }

    // A health change carries no rows: routing it into the query path would re-run the report on
    // UI-visible transitions from a provider that is failing anyway.
    let state = read_src("panels/report/state.rs");
    let poll = braced_body(&state, "if let Some(status) = refreshed");
    assert!(
        !poll.contains("requery") && !poll.contains("needs_query"),
        "a valuation health change must not trigger a report query"
    );
}

/// The valuation mode is edited in exactly one place — Settings — and every reading surface
/// learns about a change without being the one that made it.
///
/// Breakage: putting the selector back on the Analytics toolbar or the Report filters row, where
/// its label had no room and where an expert setting sits in front of every user who never needs
/// it; or landing a saved mode without waking the windows that render under it, which leaves the
/// previous conversion's numbers on screen under the new label.
#[test]
fn the_valuation_mode_selector_lives_in_settings_and_wakes_every_surface() {
    // The reading windows offer no selector of their own, and none of them writes the setting.
    // Scanned crate-wide rather than over a named list: the point is that NO surface outside
    // Settings acquires one, including hosts that do not exist yet.
    let settings_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("settings");
    let mut sources = Vec::new();
    rust_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut sources,
    );
    for path in sources {
        if path.starts_with(&settings_dir) {
            continue;
        }
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
            .replace("\r\n", "\n");
        // Any selector has to label its two options, and the only labels for them are these keys —
        // UI strings may not be literals (CONTRIBUTING.md), so a re-added control cannot avoid
        // naming them whatever it calls its own function. That is what makes this ban bite for
        // hosts that do not exist yet, rather than only for the names deleted in this change.
        assert!(
            !text.contains("general.valuation_mode"),
            "{} must not present the valuation mode: the selector lives in Settings",
            path.display()
        );
        // Reads are fine and expected — `startup.rs` and `backend/mod.rs` both need one. A WRITE
        // is what would give a surface its own copy of an application-wide setting, so each
        // occurrence must be followed by something that cannot begin an assignment: `==` counts as
        // a read, a lone `=` or a struct-literal `:` does not.
        for (at, _) in text.match_indices("report_valuation_mode") {
            let tail = text[at + "report_valuation_mode".len()..].trim_start();
            let writes =
                (tail.starts_with('=') && !tail.starts_with("==")) || tail.starts_with(':');
            assert!(
                !writes,
                "{} must not write the valuation mode: only Settings edits it",
                path.display()
            );
        }
    }

    // Settings offers both modes from one table, historical first because it is the default.
    let settings = read_src("settings/mod.rs");
    let labels = chain_between(
        &settings,
        "const VALUATION_LABELS:",
        "];",
        "the valuation-mode label table",
    );
    assert!(
        labels
            .find("ValuationMode::Historical")
            .unwrap_or(usize::MAX)
            < labels.find("ValuationMode::Current").unwrap_or(0),
        "the default conversion must be offered first"
    );

    // MoonUI-first: the row is a MoonSelect over that table, not a hand-built trigger, and it
    // carries the hint. Both caveats of the expert setting — rates come from Binance/Bybit spot,
    // and the tuner is re-valued too — live in `general.valuation_mode_hint`.
    let general = read_src("settings/general.rs");
    let tab = braced_body(&general, "pub(super) fn general_tab(");
    for needle in [
        "\"general.valuation_mode\"",
        "&self.valuation",
        "hint(&t!(\"general.valuation_mode_hint\"))",
    ] {
        assert!(
            tab.contains(needle),
            "the General tab must present the conversion setting: missing {needle}"
        );
    }
    // The row it is built from is MoonUI's select, not a hand-rolled trigger.
    assert!(
        braced_body(&general, "fn labeled_select<").contains("MoonSelect::new(state)"),
        "the General tab's enum rows must be MoonUI selects"
    );

    // A mode switch changes no rows, so neither generation moves and no open window would learn
    // about it on its own. Applying the saved mode must both aim the worker and publish the
    // revision every surface observes.
    let backend = read_src("backend/mod.rs");
    let apply = braced_body(&backend, "pub(crate) fn apply_valuation_mode(");
    for needle in [
        "set_current_wanted(",
        "self.report_revision.update(cx, |_, cx| cx.notify())",
    ] {
        assert!(
            apply.contains(needle),
            "applying the mode must reach every consumer: missing {needle}"
        );
    }
    // ...and the save path must actually call it, from INSIDE the changed-mode guard. Scoped to
    // that branch rather than asserted file-wide: an unconditional call plus the comparison
    // surviving somewhere else in the file would otherwise read as correct.
    let save = read_src("settings/apply.rs");
    let guarded = chain_between(
        &save,
        "before.report_valuation_mode != after.report_valuation_mode",
        "\n        }",
        "the Settings save's valuation-mode branch",
    );
    assert!(
        guarded.contains("apply_valuation_mode(bcx)"),
        "a Settings save must activate the mode from inside its own changed-mode guard"
    );

    // The hint must state both caveats in every language. Asserted on the TEXT, because a key that
    // resolves to one bland word would satisfy the reference check above and tell the user nothing.
    let locales =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../locales/general.yml"))
            .expect("failed to read locales/general.yml")
            .replace("\r\n", "\n");
    let hint = chain_between(
        &locales,
        "general.valuation_mode_hint:",
        "\ngeneral.",
        "the valuation-mode hint",
    );
    assert_eq!(
        hint.matches("Binance/Bybit").count(),
        3,
        "every language must name where the current rate comes from"
    );
    for tuner_word in ["тюнер", "tuning", "ajuste"] {
        assert!(
            hint.contains(tuner_word),
            "every language must state that the tuner is re-valued too: missing {tuner_word}"
        );
    }

    // The two reading windows must REQUERY on that wake, not merely repaint: the numbers change
    // even though no row did, and neither window is the writer.
    let state = read_src("panels/report/state.rs");
    // Bounded by the branch's own closing brace rather than by the comment that follows it: a
    // reworded neighbouring comment must not be able to redden this.
    let observe = chain_between(
        &state,
        "let mode = this.backend.read(cx).valuation_mode();",
        "\n            }",
        "the Report panel's valuation-mode comparison",
    );
    assert!(
        observe.contains("this.last_valuation_mode = mode") && observe.contains("request_requery("),
        "the Report panel must requery when another window changes the mode"
    );
    // And the footer must label the numbers it is SHOWING. A requery leaves the previous result on
    // screen while it runs, so a live read here would put the new mode's words under the old
    // mode's figures for as long as the query takes.
    let report_render = read_src("panels/report/render.rs");
    let footer = chain_between(
        &report_render,
        "totals::footer_facts(",
        ");",
        "the Report footer's fact assembly",
    );
    assert!(
        !footer.contains("valuation_mode()"),
        "the totals row must take its conversion from the snapshot, not from the live setting"
    );

    let analytics = read_src("analytics/mod.rs");
    let adopt = braced_body(&analytics, "fn observe_valuation_mode(");
    assert!(
        adopt.contains("self.reload(cx)"),
        "Analytics must reload when the mode changes"
    );
    // Adopting it schedules a reload, so it belongs to the poll, never to the render root — which
    // takes BOTH halves: the poll must carry it, and render must not.
    assert!(
        braced_body(&analytics, "fn observe_report_generation(")
            .contains("observe_valuation_mode("),
        "the mode must be adopted from the periodic poll"
    );
    let render = braced_body(
        &analytics,
        "fn render(&mut self, window: &mut Window, cx: &mut Context<Self>)",
    );
    for banned in ["observe_report_generation(", "observe_valuation_mode("] {
        assert!(
            !render.contains(banned),
            "the Analytics render root must not adopt the mode: found {banned}"
        );
    }
}

// Quote-scope independence is exercised by
// `panels::report::totals::tests::worker_health_is_stated_outside_every_quote_scope_branch`, which
// builds a single-currency snapshot with a stalled worker and requires the marker to lead the tail.

/// The period bar stays two named groups — presets and custom range — that wrap between
/// themselves rather than clipping either one.
///
/// Breakage this pins: flattening the groups in `analytics/toolbar.rs:period_bar` or replacing its
/// minimum height with a fixed height would split or clip controls in a narrow host.
#[test]
fn period_bar_wraps_between_its_two_control_groups() {
    let toolbar = read_src("analytics/toolbar.rs");
    let body = braced_body(&toolbar, "pub(super) fn period_bar(");

    for needle in [
        "let presets = MoonSegmentedControl::new(",
        "let custom = design::chrome_section(cx)",
        ".child(presets)",
        ".child(custom)",
    ] {
        assert!(
            body.contains(needle),
            "`period_bar` must contain {needle:?} so the presets and the custom range stay two \
             separate, atomic groups the row can wrap between"
        );
    }
    let presets_at = body
        .find("let presets = MoonSegmentedControl::new(")
        .expect("checked above");
    let custom_at = body
        .find("let custom = design::chrome_section(cx)")
        .expect("checked above");
    let child_presets_at = body.find(".child(presets)").expect("checked above");
    let child_custom_at = body.find(".child(custom)").expect("checked above");
    assert!(
        presets_at < custom_at
            && custom_at < child_presets_at
            && child_presets_at < child_custom_at,
        "the presets group must be built, then the custom group, before either joins the row, \
         so the row wraps between two complete groups rather than through a half-built one"
    );

    assert!(
        body.contains(".flex_wrap()"),
        "`period_bar` must wrap a second line instead of clipping a preset or the custom range"
    );
    assert!(
        body.contains(".min_h(design::fit_h_px(cx, 34.0, 13.0, 10.5))"),
        "`period_bar` must keep its responsive floor rather than a fixed height"
    );
    assert!(
        !body.contains(".h(design::fit_h_px(cx, 34.0, 13.0, 10.5))"),
        "a fixed row height would clip the wrapped second line instead of growing the row"
    );
}

/// The Calendar nav keeps three wrapping groups: zoom level, navigation, and badge/title.
///
/// Breakage: flattening zoom and navigation into one button run, restoring a fixed height, or
/// giving badge and title separate margins would clip controls or split the trailing pair.
///
/// It also pins the ordering source: `CalMode::ALL` is ordered for DISPLAY (Year first) and NOT as
/// the enum declares (`Day, Month, Year`), so a hand-written second list for the items or for the
/// click index would silently select a different mode than the one clicked, with no compile error.
#[test]
fn calendar_nav_wraps_between_its_control_groups() {
    let calendar = read_src("analytics/calendar/mod.rs");
    let body = braced_body(&calendar, "fn cal_nav(");

    for needle in [
        "let modes = MoonSegmentedControl::new(",
        "let nav = design::chrome_section(cx)",
        "let trailing = h_flex()",
        ".child(modes)",
        ".child(nav)",
        ".child(trailing)",
    ] {
        assert!(
            body.contains(needle),
            "`cal_nav` must contain {needle:?} so its three groups stay separate and atomic"
        );
    }
    let at = |needle: &str| body.find(needle).expect("checked above");
    assert!(
        at("let trailing = h_flex()") < at(".child(modes)"),
        "every group must be complete before any of them joins the row, so the row wraps between \
         whole groups rather than through a half-built one"
    );
    assert!(
        at(".child(modes)") < at(".child(nav)") && at(".child(nav)") < at(".child(trailing)"),
        "the row must read zoom level, then navigation, then the trailing badge and title — the \
         order the wrap divides and the order the eye follows"
    );

    assert!(
        body.contains(".flex_wrap()"),
        "`cal_nav` must wrap a second line instead of clipping a mode or a nav button"
    );
    assert!(
        body.contains(".min_h(design::fit_h_px(cx, 34.0, 13.0, 10.5))"),
        "`cal_nav` must keep its responsive floor rather than a fixed height"
    );
    assert!(
        !body.contains(".h(design::fit_h_px(cx, 34.0, 13.0, 10.5))"),
        "a fixed row height would clip the wrapped second line instead of growing the row"
    );
    assert!(
        !body.contains("div().flex_1()"),
        "a spacer child is one more thing free to take a line in a wrapping row — the trailing \
         group is pushed by its own margin instead"
    );
    assert_eq!(
        body.matches(".ml_auto()").count(),
        1,
        "exactly one margin, on the group holding BOTH the quote badge and the title — a margin \
         each would let them wrap onto separate lines"
    );
    // Check the construction and lookup sites themselves; a prose mention would not prove that
    // both sides share the same ordering source.
    assert!(
        body.contains("CalMode::ALL.map("),
        "the mode cells must be built by mapping `CalMode::ALL`, never from a literal array"
    );
    assert!(
        body.contains("CalMode::ALL.get(ix)"),
        "the click index must be resolved against the SAME ordering source the cells were built \
         from, or clicking one mode silently selects another"
    );
}

/// Every panel that owns a core selector must adopt the Profit Monitor's broadcast core filter,
/// and it must adopt it through the one shared rule.
///
/// Breakage: dropping an observer leaves that panel showing every core while its neighbours are
/// narrowed, with nothing on screen saying why the two disagree. Bypassing `apply_core_broadcast`
/// and assigning the broadcast directly is worse than a missing observer: Assets prunes a retained
/// set against its own scope, so a foreign id lands as the empty set — which that panel reads as
/// ALL cores, widening the one surface the click was meant to narrow.
#[test]
fn the_broadcast_core_filter_reaches_every_core_selector() {
    for (observer, adopter, label) in [
        ("panels/orders/mod.rs", "panels/orders/mod.rs", "Orders"),
        ("panels/alerts/mod.rs", "panels/alerts/mod.rs", "Alerts"),
        ("panels/assets/mod.rs", "panels/assets/mod.rs", "Assets"),
        (
            "panels/core_status/mod.rs",
            "panels/core_status/interactions.rs",
            "Core Status",
        ),
        (
            "panels/report/state.rs",
            "panels/report/actions.rs",
            "Report",
        ),
    ] {
        assert!(
            code_only(&read_src(observer)).contains("cx.observe(&core_filter_revision"),
            "{label} owns a core selector and must follow the broadcast core filter"
        );
        assert!(
            code_only(&read_src(adopter))
                .contains("crate::controls::apply_core_broadcast(&mut self.sel_cores"),
            "{label} must resolve the broadcast through the shared release/ignore/intersect rule"
        );
    }
    let monitor = code_only(&read_module("analytics/profit_monitor"));
    assert!(
        monitor.contains("backend.set_core_filter(next, backend_cx)")
            && monitor.contains("cx.observe(&revision"),
        "the monitor must publish the filter through Backend and repaint from the same value"
    );
}

/// Every Profit Monitor feature added after its first release must be reachable from the ⚙ popup,
/// persisted like the older choices, and drawn through the surfaces the rest of the terminal uses.
///
/// Breakage: a preference that never reaches `layout.toml` resets on every restart; a feature drawn
/// unconditionally has no way back once someone dislikes it; painting the arrival tint with
/// `with_animation` repaints the whole monitor at vblank for two seconds instead of the ten frames
/// `crate::pulse` costs; drawing the logo through gpui's `svg()` collapses a two-colour brand tile
/// into one flat silhouette; and losing the open-state write leaves an independent, taskbar-less
/// window silently gone after a restart.
#[test]
fn profit_monitor_display_preferences_and_open_state_stay_wired() {
    let source = read_module("analytics/profit_monitor");
    let settings_source = read_src("analytics/profit_monitor/settings.rs");
    let code = code_only(&source);
    let settings = code_only(&settings_source);
    let startup = code_only(&read_startup());
    let construction = code_only(braced_body(
        &source,
        "fn new(backend: Entity<Backend>, window:",
    ));
    let open = code_only(braced_body(&source, "fn open_window("));
    let mark_open = code_only(braced_body(&source, "fn mark_open("));
    let write_pref = code_only(braced_body(&settings_source, "fn write_pref("));
    let controls = code_only(braced_body(&source, "fn controls("));
    let row = code_only(braced_body(&source, "fn table_row("));
    let reload = code_only(braced_body(&source, "fn reload("));

    assert!(
        construction.contains("MonitorPrefs::restore(&backend.read(cx).layout)"),
        "the monitor constructor must restore its display preferences from the saved layout"
    );
    for key in [
        "profit_monitor_exchange_icons",
        "profit_monitor_last_trade",
        "profit_monitor_flash",
        "profit_monitor_group_sections",
        "profit_monitor_idle_cores",
        "profit_monitor_core_filter",
    ] {
        assert!(
            settings.matches(key).count() == 2,
            "{key} must be both restored and saved by its own row in the preference table"
        );
    }
    assert!(
        write_pref.contains("backend.layout_dirty = true")
            && write_pref.contains("self.invalidate_content(cx)")
            && write_pref.contains("store(&mut backend.layout, value)"),
        "a preference edit must save its OWN key, mark the layout dirty, and invalidate the          cached table"
    );
    for key in [
        "profit_monitor.settings.exchange_icons",
        "profit_monitor.settings.last_trade",
        "profit_monitor.settings.flash",
        "profit_monitor.settings.group_sections",
        "profit_monitor.settings.idle_cores",
        "profit_monitor.settings.core_filter",
    ] {
        assert!(
            settings.contains(key),
            "{key} must have its own checkbox in the settings popup"
        );
    }
    assert!(
        settings.contains("MoonCheckbox::new(")
            && settings.contains("MoonPopover::new(\"profit-monitor-settings-popover\")")
            && controls.contains("self.settings_popover(settings_trigger(self.settings_open)"),
        "the ⚙ popup must be a MoonPopover of MoonCheckboxes anchored in the control row"
    );

    assert!(
        open.matches("mark_open(&backend, cx)").count() == 3
            && mark_open.contains("backend.layout.profit_monitor_open = true")
            && mark_open.contains("backend.layout_dirty = true"),
        "immediate refocus, yielded refocus, and successful creation must record that the monitor is open"
    );
    assert!(
        code.contains("backend.layout.profit_monitor_open = false")
            && code.contains("if !backend.quitting && backend.layout.profit_monitor_open"),
        "closing by hand must clear the flag while quitting must leave it alone"
    );
    assert!(
        startup.contains("backend.layout.profit_monitor_open")
            && startup.contains("backend.persist_allowed")
            && startup.contains("crate::analytics::profit_monitor::restore("),
        "startup must reopen a monitor the previous session left open, through the non-activating \
         path and never during a diagnostic run"
    );
    assert!(
        !startup.contains("profit_monitor::open("),
        "restoring through `open` activates the window and steals startup focus from Main"
    );

    assert!(
        row.contains("crate::pulse::with_arrival_tint(") && !code.contains("with_animation"),
        "the arrival tint must be the shared pulse-driven one, not a vblank animation"
    );
    assert!(
        reload.contains("self.rebaseline_arrivals()")
            && code_only(braced_body(&source, "fn start_clock_refresh("))
                .contains("this.rebaseline_arrivals()"),
        "a query change — including a period boundary — replaces every value at once and must not \
         read as a table full of arrivals"
    );
    assert!(
        code.contains("use crate::media::exchange_logos::exchange_logo;")
            && code.contains("exchange_logo(brand)")
            && !code.contains("svg()"),
        "exchange logos must come from the shared rasterizer rather than gpui's monochrome svg()"
    );
    assert!(
        code.contains(".entry(brand)") && code.contains(".or_insert_with(|| exchange_logo(brand))"),
        "logos must be resolved once per distinct brand, not once per row: the render path pays \
         a global-cache lock for every one of them"
    );
}

/// Every one of the five Report toolbar filter mutators must route its change through
/// `persist_filters`, or that member appears to work until the panel or window is reopened and
/// the edit is silently gone.
///
/// `moon-ui-gpui` is a binary crate, so these click handlers live in GPUI closures no integration
/// test can call — the wiring can only be pinned at the source level, the same technique as
/// `tuner_field_checkboxes_persist_every_change`. The pure decode/assemble half of the same
/// contract is covered by unit tests in `panels/report/tests.rs`, which this file cannot reach.
///
/// Mutation: deleting any one `self.persist_filters(` call below reddens its own assertion.
#[test]
fn every_report_filter_mutator_persists_its_change() {
    let actions = code_only(&read_src("panels/report/actions.rs"));
    for signature in [
        "pub(super) fn set_side(&mut self, s: SideFilter, cx: &mut Context<Self>) {",
        "pub(super) fn set_period(&mut self, p: Period, cx: &mut Context<Self>) {",
        "pub(super) fn set_kind(&mut self, k: ReportKind, cx: &mut Context<Self>) {",
        "pub(super) fn set_deleted_only(&mut self, on: bool, cx: &mut Context<Self>) {",
        "pub(super) fn set_show_open(&mut self, on: bool, cx: &mut Context<Self>) {",
    ] {
        let body = code_only(braced_body(&actions, signature));
        assert!(
            body.contains("self.persist_filters("),
            "{signature} changes a Report toolbar filter and must persist it"
        );
    }
}
