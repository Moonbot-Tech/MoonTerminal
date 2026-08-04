//! Shell chrome: the status bar, the header cluster, the trading toolbar, the shared core
//! selectors, the log panel's exchange headers and the core-status table.

use super::support::*;

/// Protects durable per-group Main-core selection and authoritative target ownership.
///
/// The plausible edit is restoring a separate `trade_core_override` map or writing the current
/// Main target from a child stack or on every sync. Restart would forget manual choices, while a
/// hidden, detached, or stale cross-group chart could overwrite the visible anchor and route
/// trading hotkeys to the wrong core.
#[test]
fn active_trade_core_selection_is_layout_backed_and_sticky() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let main = fs::read_to_string(root.join("main.rs")).unwrap();
    let startup = fs::read_to_string(root.join("startup.rs")).unwrap();
    let backend = fs::read_to_string(root.join("backend").join("mod.rs")).unwrap();
    let chrome = fs::read_to_string(root.join("chrome").join("terminal_chrome.rs")).unwrap();
    let chart_tabs = fs::read_to_string(root.join("chart_tabs").join("mod.rs")).unwrap();
    let main_stack = fs::read_to_string(root.join("chart_tabs").join("main_stack.rs")).unwrap();
    let ingest = fs::read_to_string(root.join("chart_tabs").join("ingest.rs")).unwrap();
    let windows = fs::read_to_string(root.join("chart_tabs").join("windows.rs")).unwrap();
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
    let production = format!("{main}\n{startup}\n{backend}\n{chrome}");

    assert!(
        !production.contains("trade_core_override"),
        "the superseded Backend-only active-core path must stay removed"
    );
    assert!(
        layout.contains("pub active_trade_core_by_group: HashMap<String, u64>")
            && startup.contains("layout: layout.clone(),"),
        "the stable per-group core UID must live in the layout loaded at startup"
    );
    assert!(
        chart_tabs.contains(
            "this.sync_active_scale(cx);\n        this.initialize_main_chart_target(cx);"
        ),
        "restored Main state must initialize its runtime target without replacing the saved core"
    );
    let initial_target_start = backend
        .find("pub(crate) fn initialize_main_chart_target")
        .expect("initial Main target setter must exist");
    let initial_target_end = backend[initial_target_start..]
        .find("pub(crate) fn set_main_chart_target")
        .map(|offset| initial_target_start + offset)
        .expect("runtime Main target setter must follow initialization");
    let initial_target_setter = &backend[initial_target_start..initial_target_end];
    assert!(
        initial_target_setter.contains("self.store_main_chart_target(group, target);")
            && !initial_target_setter.contains("set_active_trade_core"),
        "startup target initialization must preserve the durable manual header selection"
    );

    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);
    let mut target_publishers = Vec::new();
    for path in sources {
        let source = fs::read_to_string(&path).unwrap();
        for (line_ix, line) in source.lines().enumerate() {
            if line.contains(".set_main_chart_target(") {
                target_publishers.push(format!("{}:{}", path.display(), line_ix + 1));
            }
        }
    }
    assert_eq!(
        target_publishers.len(),
        1,
        "ChartTabs must be the sole Main-target publisher; found: {target_publishers:?}"
    );
    assert!(
        chart_tabs.contains("b.set_main_chart_target(&self.group, target)")
            && !main_stack.contains(".set_main_chart_target("),
        "the sole publisher must compute the visible anchor-aware target at ChartTabs level"
    );
    assert!(
        chart_tabs.contains("cx.observe(&main, |this, _main, cx| {")
            && chart_tabs.contains("this.sync_main_chart_target(cx);")
            && ingest.contains("self.watch_regular_stack_target(&panel, cx);")
            && windows.contains("this.watch_regular_stack_target(&panel, cx);"),
        "Main plus runtime-created and restored ordinary stacks must refresh the authoritative target"
    );
    let detach_start = windows
        .find("pub(super) fn detach")
        .expect("chart detach handler must exist");
    let detach_end = windows[detach_start..]
        .find("fn open_chart_window")
        .map(|offset| detach_start + offset)
        .expect("detached-window opener must follow the detach handler");
    let detach_handler = &windows[detach_start..detach_end];
    assert!(
        detach_handler.contains("if self.active == tab {")
            && detach_handler.contains("self.active = Tab::Main;")
            && detach_handler.contains("self.sync_main_chart_target(cx);"),
        "detaching the active comparison tab must replace its target with visible Main"
    );

    let target_start = backend
        .find("pub(crate) fn set_main_chart_target")
        .expect("Main target setter must exist");
    let target_end = backend[target_start..]
        .find("pub(crate) fn main_chart_target")
        .map(|offset| target_start + offset)
        .expect("Main target getter must follow its setter");
    let target_setter = &backend[target_start..target_end];
    assert!(
        target_setter.contains("if prev_core != Some(*new_core)")
            && target_setter.contains("self.set_active_trade_core(group, *new_core);")
            && target_setter
                .matches("self.set_active_trade_core(group, *new_core);")
                .count()
                == 1,
        "same-core Main syncs must preserve a manual choice; only a core change may replace it"
    );
    assert!(
        target_setter
            .contains("target.filter(|(core, _)| self.core_belongs_to_group(group, *core))"),
        "invalid incoming Main targets must be discarded before persistence"
    );

    let target_getter_start = target_end;
    let target_getter_end = backend[target_getter_start..]
        .find("pub(crate) fn set_main_open_markets")
        .map(|offset| target_getter_start + offset)
        .expect("Main open-market setter must follow the target getter");
    let target_getter = &backend[target_getter_start..target_getter_end];
    assert!(
        target_getter.contains(".filter(|(core, _)| self.core_belongs_to_group(group, *core))"),
        "every direct Main-target consumer must receive None after a core leaves the group"
    );

    let active_start = backend
        .find("pub(crate) fn active_trade_core")
        .expect("active trade-core resolver must exist");
    let active_end = backend[active_start..]
        .find("pub(crate) fn set_active_trade_core")
        .map(|offset| active_start + offset)
        .expect("active trade-core setter must follow its resolver");
    let active_resolver = &backend[active_start..active_end];
    assert!(
        active_resolver.contains("self.layout.active_trade_core_by_group.get(group)")
            && active_resolver.contains("self.core_belongs_to_group(group, core)")
            && active_resolver.contains("self.main_chart_target(group)"),
        "saved cores must be used only while a live session still belongs to the same group"
    );

    let setter_start = active_end;
    let setter_end = backend[setter_start..]
        .find("/// Refresh the cached fallback ticker")
        .map(|offset| setter_start + offset)
        .expect("ticker method must follow the active trade-core setter");
    let active_setter = &backend[setter_start..setter_end];
    assert!(
        active_setter.contains("if !self.core_belongs_to_group(group, core)")
            && active_setter.contains(".active_trade_core_by_group")
            && active_setter.contains(".insert(group.to_string(), core);")
            && active_setter.contains("self.layout_dirty = true;")
            && chrome.contains("b.set_active_trade_core(&group, id);"),
        "manual selection must update layout, mark persistence dirty, and use the shared setter"
    );
}

#[test]
fn status_bar_states_no_latency_it_did_not_measure() {
    // The banned "ping 32ms" readout is a literal: nothing in the code measures RTT, and in a
    // trading terminal an invented latency is worse than none, because people act on it.
    // The ban is narrow on purpose — it forbids bringing back the PLACEHOLDER specifically, while
    // a real RTT would arrive through a format! over live metrics and never match it.
    let text = read_src("shell/status_bar.rs");
    for banned in ["MoonStatusItem::new(\"ping\")", "\"32ms\""] {
        assert!(
            !text.contains(banned),
            "status_bar.rs must not hard-code a latency readout: found {banned}"
        );
    }
}

#[test]
fn header_ticker_popup_accounts_for_the_clock_beside_it() {
    // The ticker sits to the LEFT of the clock, while its popup is positioned by hand from the
    // window's right edge inward — so the offset has to include the clock's width. The plausible
    // edit this catches is dropping the clock-width term while simplifying the popup offset, which
    // makes the popup silently open under the wrong element.
    //
    // It pins the COUPLING, not the arithmetic: this harness reads source text and has no gpui
    // `App`, so the actual pixel offset cannot be evaluated here and alignment stays a
    // confirm-on-a-real-window item. Nor does it endorse the hand-positioning — delete this test
    // when the popup moves to a MoonUI Root-owned anchored layer and the offset goes away.
    let text = read_src("shell/ticker.rs");
    assert!(
        text.contains("header_clock_width"),
        "ticker.rs positions its popup from the window edge, so it must account for the clock's \
         measured width — see terminal_chrome's header cluster order"
    );

    // And the measurement must still agree with what the clock draws. Asserting on the two
    // BODIES, not merely that the functions exist: either one could quietly stop calling
    // `clock_parts` and reimplement the time and city-code strings for itself — or reintroduce a
    // rule that hides the code, which changes the clock's width — and that is exactly the drift
    // that puts the popup off its trigger, while a name-only check stays green through it.
    let clock = read_src("chrome/clock.rs");
    for signature in ["fn header_clock_width", "fn header_clock("] {
        let body = fn_body(&clock, signature);
        assert!(
            body.contains("clock_parts("),
            "clock.rs: {signature} must derive its strings from clock_parts — the renderer and the \
             width measurement share one model so they cannot drift apart"
        );
    }
}

/// Protect every shared multi-select core picker from regressing to passive exchange labels.
///
/// Regression target: replacing one panel's `core_combo` batch callback with an individual-only
/// selector leaves that panel's exchange headers inert while Analytics continues to work. The
/// handler assertions also catch moving refresh work outside the helper's changed-selection guard
/// or duplicating it, which would requery or rebuild once per exchange member.
#[test]
fn shared_core_selectors_batch_exchange_changes_once() {
    let selector_cases = [
        (
            "Analytics",
            "analytics/toolbar.rs",
            "fn core_combo(",
            "t.toggle_exchange_cores(exchange_cores, c);",
        ),
        (
            "Orders",
            "panels/orders/controls.rs",
            "pub(super) fn source_combo(",
            "t.toggle_exchange_cores(exchange_cores, c);",
        ),
        (
            "Report",
            "panels/report/controls.rs",
            "pub(super) fn core_combo(",
            "t.toggle_exchange_cores(exchange_cores, c);",
        ),
        (
            "Assets",
            "panels/assets/table.rs",
            "pub(super) fn core_combo(",
            "t.toggle_exchange_cores(exchange_cores, c);",
        ),
        (
            "Core Status",
            "panels/core_status/mod.rs",
            "fn core_bar(",
            "t.toggle_exchange_cores(exchange_cores, c);",
        ),
    ];
    for (panel, path, signature, callback) in selector_cases {
        let source = read_src(path);
        let body = braced_body(&source, signature);
        assert!(
            body.contains("crate::controls::core_combo(") && body.contains(callback),
            "{panel} must wire its exchange row to one batch-selection callback"
        );
    }

    let shared = read_src("controls/core_combo.rs");
    let shared_body = braced_body(&shared, "pub(crate) fn core_combo<");
    assert!(
        shared_body.contains("MoonMenuItem::action_label(")
            && shared_body.contains("if exchange.is_some()")
            && shared_body.contains("let exchange_cores = section_core_ids(&members);")
            && shared_body.contains("MoonMenuItem::label(exchange_label)"),
        "known exchanges must submit every section member while the unknown section remains a label"
    );

    let handler_cases: [(&str, &str, &str, &[&str]); 4] = [
        (
            "Orders",
            "panels/orders/mod.rs",
            "pub(super) fn toggle_exchange_cores(",
            &["self.rebuild_cache(", "cx.notify()"],
        ),
        (
            "Report",
            "panels/report/actions.rs",
            "pub(super) fn toggle_exchange_cores(",
            &["self.request_requery("],
        ),
        (
            "Assets",
            "panels/assets/mod.rs",
            "pub(super) fn toggle_exchange_cores(",
            &["self.rebuild_cache(", "cx.notify()"],
        ),
        (
            "Core Status",
            "panels/core_status/interactions.rs",
            "pub(super) fn toggle_exchange_cores(",
            &["self.rebuild_cache(", "cx.notify()"],
        ),
    ];
    for (panel, path, signature, effects) in handler_cases {
        let source = read_src(path);
        let body = braced_body(&source, signature);
        assert!(
            body.contains("if crate::controls::toggle_exchange_cores("),
            "{panel} must skip downstream work for a stale-only exchange batch"
        );
        let changed_branch = braced_body(body, "if crate::controls::toggle_exchange_cores(");
        assert!(
            !changed_branch.contains("for ")
                && !changed_branch.contains("while ")
                && !changed_branch.contains(".for_each("),
            "{panel} must not repeat downstream work per exchange member"
        );
        for effect in effects {
            assert_eq!(
                changed_branch.matches(effect).count(),
                1,
                "{panel} must perform `{effect}` once inside the changed-selection guard"
            );
            assert_eq!(
                body.matches(effect).count(),
                changed_branch.matches(effect).count(),
                "{panel} must not perform `{effect}` outside the changed-selection guard"
            );
        }
    }
}

#[test]
fn status_bar_connection_and_license_are_localized() {
    // Neither caption is on the deliberately-untranslated list in locales/README.md, so an English
    // literal here is a localization regression rather than policy. The status bar does carry
    // entries from that list — the ticks/book/fps/CPU/RAM metrics string and the PRO/FREE plan
    // names — which is exactly why the ban names these two keys instead of banning English text.
    let text = read_src("shell/status_bar.rs");
    for key in ["status.connection", "status.license"] {
        assert!(
            text.contains(&format!("t!(\"{key}\"")),
            "status_bar.rs must render its label through t!(\"{key}\")"
        );
    }
    for banned in ["\"Connection:", "\"License:"] {
        assert!(
            !text.contains(banned),
            "status_bar.rs must not hard-code the English label {banned}"
        );
    }
}

/// The three status groups must remain visibly distinct while retaining every live RAM metric.
///
/// Plausible production regression: in `shell/status_bar.rs:Shell::status_bar`, replace the
/// `group_separator()` immediately after `license_text` with `separator()`. The named group-count
/// assertion reddens, because connection/license and BOOK/FPS would collapse into one dotted run.
#[test]
fn status_bar_keeps_three_glanceable_groups() {
    let text = read_src("shell/status_bar.rs");
    let left_items = text
        .split_once(".items([")
        .expect("status bar must define its left items")
        .1
        .split_once(".right_items(right_items)")
        .expect("status bar must keep actions in the right region")
        .0;
    let groups = left_items
        .split("MoonStatusItem::group_separator()")
        .collect::<Vec<_>>();
    assert_eq!(
        groups.len(),
        3,
        "status_bar.rs must keep exactly three ordered left-side groups"
    );
    for (group, required) in [
        (groups[0], ["status_text", "license_text"].as_slice()),
        (
            groups[1],
            ["\"BOOK\"", "book_levels", "\"FPS\"", "fps"].as_slice(),
        ),
        (
            groups[2],
            [
                "\"CPU APP/SYS\"",
                "snap.cpu_process",
                "snap.cpu_system",
                "\"GPU\"",
                "snap.gpu_process",
                "\"RAM\"",
                "snap.mem_mb",
                "snap.mem_delta_mb",
            ]
            .as_slice(),
        ),
    ] {
        for marker in required {
            assert!(
                group.contains(marker),
                "status-bar group must contain {marker}"
            );
        }
    }
}

/// Protects the row viewport both log surfaces scroll sideways in.
///
/// The plausible edits are dropping the list's own `Hidden` scrollbar — which would ride the right
/// edge of the CONTENT, off screen, while a second bar appeared over the rows — and softening the
/// horizontal bar to a visibility that only shows it while scrolling. The wheel is deliberately
/// restricted to one axis, so a bar nobody can see is a tail nobody can reach.
#[test]
fn log_row_viewports_keep_a_reachable_horizontal_scrollbar() {
    let line_list = read_src("panels/line_list.rs");
    let viewport = braced_body(&line_list, "pub(crate) fn hscroll_viewport(");
    assert!(
        viewport.contains("MoonScrollAxis::Horizontal")
            && viewport.contains("MoonScrollbarVisibility::Always")
            && viewport.contains(".restrict_scroll_to_axis()"),
        "the shared row viewport must keep a permanently visible horizontal scrollbar"
    );
    let log_view = read_src("panels/log/view.rs");
    let trade_log_view = read_src("panels/report/trade_log/view.rs");
    for (surface, text) in [
        ("panels/log/view.rs", &log_view),
        ("panels/report/trade_log/view.rs", &trade_log_view),
    ] {
        assert!(
            text.contains(".scrollbar_visibility(MoonScrollbarVisibility::Hidden)")
                && text.contains("line_list::hscroll_viewport(")
                // Not merely hidden: a bar drawn HERE would ride the content and leave the viewport
                // to draw a second one over the rows.
                && !text.contains(".horizontal_scrollbar(")
                && !text.contains(".vertical_scrollbar("),
            "{surface} must hand its scrollbars to the shared viewport, not draw its own"
        );
    }
    // Following the tail has to yield to the bar as well as to the wheel: a drag that moved the
    // list would otherwise be undone by the next reload. The body is read, not just the call, or an
    // emptied listener would still pass on the wheel handler's own `pause_follow`.
    let grab = log_view
        .split_once(".capture_any_mouse_down(")
        .map(|(_, rest)| rest)
        .unwrap_or_default();
    assert!(
        grab.split("}))")
            .next()
            .unwrap_or_default()
            .contains("pause_follow"),
        "grabbing a Log scrollbar must pause tail following"
    );
}

/// `panels/log/controls.rs:source_combo` replacing the known exchange action with a label would
/// leave the Log tab's exchange headings inert even though the shared core selectors still work.
#[test]
fn log_exchange_headers_select_a_live_exchange_aggregate() {
    let controls = read_src("panels/log/controls.rs");
    let combo = braced_body(&controls, "pub(super) fn source_combo(");
    let known_exchange = braced_body(combo, "if exchange.is_some()");
    let unknown_exchange = braced_body(combo, "} else {");
    assert!(
        known_exchange.contains("MoonMenuItem::action_label(")
            && known_exchange.contains("this.set_source(LogSource::Exchange(source), cx);")
            && !known_exchange.contains("MoonMenuItem::label(exchange_label)")
            && unknown_exchange.contains("MoonMenuItem::label(exchange_label)")
            && !unknown_exchange.contains("MoonMenuItem::action_label("),
        "known Log exchange headers must select an exchange source while Unknown stays passive"
    );

    let panel = read_src("panels/log/mod.rs");
    let gather = braced_body(&panel, "fn gather(");
    let reload = braced_body(&panel, "fn reload_rows(");
    // The two places that must treat an exchange source exactly like the aggregate: the reload path
    // in the panel, and the file-selector visibility in its element tree. They live in separate
    // files and are counted separately — a sum would let one arm vanish while the other doubled.
    let view = read_src("panels/log/view.rs");
    let arms = |text: &str| {
        text.matches("LogSource::Aggregate | LogSource::Exchange(_)")
            .count()
    };
    assert!(
        panel.contains("Exchange(String)")
            && gather.contains("LogSource::Exchange(_)")
            && gather.contains("exchange_membership")
            && arms(&panel) == 1
            && arms(&view) == 1
            && reload.contains("render::exchange_core_ids(")
            && reload.contains("exchange_membership_changed(")
            && reload.contains("self.following() || membership_changed"),
        "the selected Log exchange source must gather only its current live membership"
    );

    let render = read_src("panels/log/render.rs");
    let signature = braced_body(&render, "pub(super) fn log_sig(");
    assert!(
        signature.contains("LogSource::Exchange(exchange)")
            && signature.contains("selected_core_log_sig(")
            && render.contains("pub(super) fn exchange_chart_candidates"),
        "exchange rows, refresh signatures, and chart candidates must share exchange scope"
    );
}

#[test]
fn toolbar_row_budget_counts_every_rule_it_draws() {
    // `controls::toolbar::row_fit` decides which of the row's labels collapse by summing the row's
    // fixed width, and it restates the row's structure to do so — the rule count among it. The row
    // itself is built 250 lines below, so the two are coupled only by hand.
    //
    // Plausible future edit, and the reason this test exists: a sixth section is added to the row
    // with its `design::chrome_divider` sibling, and `row_fit` is not touched. Nothing fails to
    // compile, and at any comfortable window width nothing looks wrong — the budget is short by one
    // rule plus its gaps, so at some narrow width a label stays on screen after the row has already
    // outgrown the window and the trailing window buttons clip off the right edge. That is the
    // exact failure `row_fit` exists to prevent, so an undercount defeats the mechanism silently.
    //
    // Static, because this harness has no gpui `App` and cannot lay a row out; it pins the COUPLING
    // rather than any pixel value, the same way the header ticker's popup test does.
    let text = read_src("controls/toolbar.rs");
    let drawn = fn_body(&text, "pub fn toolbar(")
        .matches("design::chrome_divider(cx, p)")
        .count();
    let budgeted = fn_body(&text, "fn row_fit(")
        .split_once("let rules = ")
        .and_then(|(_, rest)| rest.split_once(';'))
        .and_then(|(value, _)| value.trim().trim_end_matches("f32").parse::<f32>().ok())
        .expect("row_fit must state its rule count as `let rules = <number>;`");

    assert_eq!(
        budgeted, drawn as f32,
        "row_fit budgets {budgeted} rules while toolbar() draws {drawn}: the collapse thresholds \
         are off by the difference, which shows up only as the trailing cluster clipping at a \
         narrow window"
    );
}

/// `core_status/table.rs:core_status_row`, `server_view.rs:server_row` and `core_row` must keep
/// each protocol-v4 field bound to its correctly scoped UI metric. Swapping process/system CPU or
/// process/free memory compiles but gives the operator a believable number with the wrong scope.
#[test]
fn core_status_table_binds_scoped_telemetry_columns() {
    let text = read_src("panels/core_status/table.rs");
    let flat_row = braced_body(&text, "fn core_status_row(");
    let server = read_src("panels/core_status/server_view.rs");
    let server_row = braced_body(&server, "fn server_row(");
    let process_row = braced_body(&server, "fn core_row(");

    for key in [
        "core_status.col.server",
        "core_status.col.cpu_proc",
        "core_status.col.cpu_sys",
        "core_status.col.cpus",
    ] {
        assert!(
            text.contains(key),
            "core_status/table.rs must bind the scoped telemetry column `{key}`"
        );
    }
    for banned in [
        "core_status.col.cpu_avg",
        "core_status.col.mem_app",
        "core_status.col.mem_sys",
        "core_status.col.free_page",
    ] {
        assert!(
            !text.contains(banned),
            "core_status/table.rs must not resurrect the removed column `{banned}` (v4 \
             KernelHealth has no source for it; `cpu_avg` mislabels machine CPU as an average)"
        );
    }
    for binding in [
        "percent(sys.process_cpu_percent)",
        "percent(sys.system_cpu_percent)",
        "memory_u16(sys.used_memory_mb)",
        "memory_u16(sys.free_physical_memory_mb)",
        "count(sys.logical_cpu_count)",
    ] {
        assert!(
            flat_row.contains(binding),
            "the Flat Core Status row lost the scoped telemetry binding `{binding}`"
        );
    }
    for binding in [
        "cpu_load(group.system_cpu_percent, group.logical_cpu_count)",
        "memory_free(group.process_memory_mb, group.free_physical_memory_mb)",
    ] {
        assert!(
            server_row.contains(binding),
            "the By IP server row lost the scoped telemetry binding `{binding}`"
        );
    }
    for binding in [
        "percent(core.sys.process_cpu_percent)",
        "memory_u16(core.sys.used_memory_mb)",
    ] {
        assert!(
            process_row.contains(binding),
            "the By IP process row lost the scoped telemetry binding `{binding}`"
        );
    }
}

/// `core_status/server_view.rs:grouped_server_view` must not capture a strong
/// `CoreStatusView` entity in MoonTree's retained renderer; replacing the weak
/// owner with `let tree_owner = cx.entity();` leaks the panel and its observers.
#[test]
fn core_status_tree_renderer_holds_a_weak_owner() {
    let text = read_src("panels/core_status/server_view.rs");

    assert!(
        text.contains("let weak_view = cx.entity().downgrade();")
            && text.contains("weak_view.upgrade()"),
        "Core Status tree callbacks must downgrade and conditionally upgrade their view owner"
    );
    assert!(
        !text.contains("let tree_owner = cx.entity();"),
        "Core Status MoonTree retained a strong panel handle and created an ownership cycle"
    );
}

/// `core_status/mod.rs` must keep telemetry repaints throttled to at most once per second and show
/// CPU AVERAGED over the window, not the last flickering sample. Removing the throttle floods the
/// panel with repaints; dropping the average makes the number unreadable.
#[test]
fn core_status_throttles_repaints_and_averages_cpu() {
    let text = read_src("panels/core_status/mod.rs");

    assert!(
        text.contains("now - this.last_repaint_ms < 1000") && text.contains("return;"),
        "Core Status must gate ALL telemetry work to at most once per second (early-return on \
         faster drains), not just the repaint"
    );
    // Detection and CPU averaging moved to the backend engine; the panel must read the smoothed
    // value from it rather than the raw last sample. `collect` lives in the cache-pipeline module.
    let cache = read_src("panels/core_status/cache.rs");
    assert!(
        cache.contains("b.warn.avg_cpu("),
        "Core Status must display CPU smoothed by the backend warning engine, not the raw last sample"
    );
    let engine = read_src("backend/core_warn.rs");
    assert!(
        engine.contains("fn averaged(") && engine.contains("CPU_WINDOW_SECS"),
        "the warning engine must average CPU over the window"
    );
}

/// Protects the market popups from letting the wheel reach whatever they cover.
///
/// The plausible edit is dropping `occlude()` while reshuffling a popup's chrome — it reads as a
/// no-op next to the `stop_propagation` already there. The chart reads the wheel through an
/// ordinary gpui handler gated on its own hitbox, so without the occluder a scroll over the coin
/// list also rescales the chart behind it, and a scroll over the header ticker reaches the
/// surface under the header. Neither shows up in any unit test: it is visible only in a running
/// build.
#[test]
fn market_popups_occlude_the_wheel_from_the_surface_behind() {
    let coin_search = read_src("controls/coin_search.rs");
    let ticker = read_src("shell/ticker.rs");

    let popup = braced_body(&coin_search, "pub(crate) fn render_popup<F, G, H>(");
    assert!(
        popup.contains(".occlude()"),
        "the coin-search popup must occlude, or the wheel over its results rescales the chart \
         behind them"
    );

    let layers = braced_body(&ticker, "pub(super) fn ticker_popup_layers(");
    assert!(
        layers.contains(".occlude()"),
        "the header ticker's popup box must occlude: its caption and query input sit ABOVE the \
         occluding results list, so that band would still pass the wheel through"
    );
}

/// Protects every stack card's gutter from being decided by a literal at the call site.
///
/// The plausible edit is passing `true` (or `!fullscreen`) straight into `chart_stack_card`.
/// The gutter is 8px of panel colour drawn BELOW the card, and it also
/// shortens the chart body by that much — so a stack holding one tile shows an empty strip under
/// the chart, and Main's right-click gesture moved the chart vertically for no visible reason.
/// Both stacks must route the decision through the one predicate that knows how many tiles there
/// are.
#[test]
fn stack_cards_take_their_gutter_from_the_shared_decision() {
    let stack = read_src("chart_tabs/stack.rs");
    assert!(
        stack.contains("pub(super) fn tile_gutter("),
        "the shared gutter decision must live in stack.rs beside the card that draws it"
    );

    for rel in ["chart_tabs/main_stack.rs", "chart_tabs/add_stack.rs"] {
        let source = read_src(rel);
        let call = chain_between(&source, "chart_stack_card(", ");", rel);
        assert!(
            call.contains("tile_gutter("),
            "{rel}: the chart_stack_card call must take its gutter from tile_gutter, not a literal"
        );
    }
}

/// Protects the Main tab row from appearing where it says nothing, and its handlers from
/// addressing charts by a number that moves.
///
/// Two plausible edits, both silent. Rendering the row unconditionally costs a strip of chart
/// height to name a market the card header already names. And keeping the click handlers' snapshot
/// as indices — the shape the tab strip above it uses, where tabs do not renumber — would let an
/// expiring chart or a comparison lock reordering the stack slide a different market under a
/// perfectly in-range index, so a click selects, fullscreens or CLOSES the wrong chart.
#[test]
fn the_main_tab_row_is_gated_and_addresses_charts_by_identity() {
    let main_stack = read_src("chart_tabs/main_stack.rs");

    let row = braced_body(&main_stack, "fn render_tab_row(");
    assert!(
        row.contains("live.len() < 2"),
        "the row must be suppressed below two charts"
    );
    assert!(
        row.contains("Rc<Vec<(CoreId, String)>>"),
        "its handlers must snapshot chart IDENTITY, never indices"
    );

    // Both gestures funnel through one selection method, and THAT is where the identity is turned
    // into a current index — at the moment the handler fires, not when the row was drawn.
    let select = braced_body(&main_stack, "fn select_market(");
    assert!(
        select.contains("self.index_of("),
        "select_market must resolve the identity to a current index when it fires"
    );
    for name in ["fn focus_market(", "fn fullscreen_market("] {
        let body = braced_body(&main_stack, name);
        assert!(
            body.contains("self.select_market("),
            "{name} must go through the shared selection, not resolve or assign on its own"
        );
    }

    let close_at = braced_body(&main_stack, "fn close_at(");
    assert!(
        close_at.contains("remap_active_index("),
        "close_at must re-resolve the active chart by identity: removing an earlier entry shifts \
         every index after it"
    );
    assert!(
        close_at.contains("remove_chart_at("),
        "and must release the panel and any comparison lock through the shared removal, not a \
         second copy of it"
    );
}

/// Protects every bounded `MoonTabStrip` from escaping the box that is meant to hold it.
///
/// The plausible edit is wrapping a strip in a plain `div` — nothing about the call site suggests
/// otherwise. But a strip given explicit `bounds` makes its OWN root absolute and positions itself
/// against the nearest positioned ancestor, so an unpositioned wrapper silently hands it to
/// whichever ancestor happens to be relative. In this panel that is `ChartTabs`'s root, and the row
/// then paints over the Main/Add tab strip at the top of the window instead of sitting above the
/// chart.
#[test]
fn a_bounded_tab_strip_sits_in_a_positioned_wrapper() {
    for (rel, signature) in [
        ("chart_tabs/main_stack.rs", "fn render_tab_row("),
        (
            "chart_tabs/strip.rs",
            "fn render(&mut self, window: &mut Window",
        ),
    ] {
        let source = read_src(rel);
        let body = braced_body(&source, signature);
        // Comments are stripped first, and that is load-bearing: the comment explaining WHY the
        // wrapper is positioned names `.relative()` itself, so a raw search over the body stayed
        // true with the actual call deleted — the assertion passed on the very edit it exists to
        // catch.
        let code: String = body
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            code.contains(".bounds(MoonRect::new("),
            "{rel}: expected a bounded tab strip here"
        );
        assert!(
            code.contains(".relative()"),
            "{rel}: a bounded MoonTabStrip must live inside a positioned wrapper, or it anchors to \
             an unrelated ancestor"
        );
    }
}

/// Protects every path that removes a Main chart from doing its own teardown.
///
/// The plausible edit is independently popping the entry and closing its panel in each path.
/// Each chart also owns state OUTSIDE its own entry — a comparison lock it may be
/// the anchor of, and the published list of markets Orders highlights — and a removal that skips
/// those leaves surviving charts locked to a leader that is gone, or Orders pointing at charts that
/// no longer exist. It is invisible until a user hits Shift+Escape or waits out the idle timer.
#[test]
fn every_main_chart_removal_goes_through_the_shared_teardown() {
    let main_stack = read_src("chart_tabs/main_stack.rs");

    for name in ["fn close_active(", "fn close_at(", "fn prune_idle("] {
        let body = braced_body(&main_stack, name);
        assert!(
            body.contains("remove_chart_at("),
            "{name} must release the chart through the shared removal"
        );
        assert!(
            !body.contains("self.charts.remove("),
            "{name} must not take a chart out of the stack itself"
        );
    }

    // `close_all` drains the whole stack rather than removing one entry, so it clears the same
    // state inline; what it must not do is forget it.
    let close_all = braced_body(&main_stack, "fn close_all(");
    assert!(
        close_all.contains("self.compare_anchor = None")
            && close_all.contains("sync_backend_open_markets("),
        "close_all must drop the comparison anchor and republish the open-market list"
    );

    // The idle sweep must re-resolve the selection by identity: an EARLIER chart expiring shifts
    // every later index, so a clamped number selects somebody else's market.
    let prune = braced_body(&main_stack, "fn prune_idle(");
    assert!(
        prune.contains("remap_active_index("),
        "prune_idle must re-resolve the active chart by identity, not clamp its old index"
    );
}

/// Protects the coin-search popup's multi-select hint from wrapping onto a second line.
///
/// The plausible edit is dropping `whitespace_nowrap` while restyling the note, or adding a longer
/// translation and assuming the box will cope. It will not: wrapped, the hint doubles the popup's
/// header and pushes the first result out of view. The popup's width is font-scaled while the
/// dictionary values are not, so the guarantee has to be structural — the line clips rather than
/// folds — and nothing else in the popup would fail if it folded.
#[test]
fn the_multi_select_hint_clips_instead_of_wrapping() {
    let coin_search = read_src("controls/coin_search.rs");
    let popup = braced_body(&coin_search, "pub(crate) fn render_popup<F, G, H>(");
    let hint = chain_between(
        &popup,
        "if multi_select {",
        "chart.coin.multi_hint",
        "the multi-select hint",
    );

    assert!(
        hint.contains(".whitespace_nowrap()") && hint.contains(".overflow_hidden()"),
        "the hint must be pinned to one line and clip: wrapped, it hides the first result"
    );
}

/// Protects the movers suggestion from offering one market once per core that can open it.
///
/// The plausible edit is restoring the `flat_map` over every consumer core — it looks like the
/// generous choice, and it is what the typed search does. But a mover is a property of the MARKET,
/// so on a config where dozens of cores share an exchange the top of eight becomes the same coin
/// repeated down the entire list, which is what the section is for reading past.
#[test]
fn the_movers_suggestion_offers_each_market_once() {
    let coin_search = read_src("controls/coin_search.rs");
    let suggest = braced_body(&coin_search, "pub(crate) fn suggest_volatile(");
    let code: String = suggest
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        code.contains(".first()?"),
        "each mover must resolve to the FIRST core that can open it"
    );
    assert!(
        !code.contains(".flat_map(|mover|"),
        "and must not fan a mover out across every consuming core"
    );
}
