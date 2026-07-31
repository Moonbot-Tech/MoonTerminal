//! The three window classes carry deliberately different OS options, and FireTest stays a
//! runtime scenario rather than a static assertion. Prose: `docs/WINDOWING.md`.

use super::support::*;

#[test]
fn terminal_windowing_separates_detached_panel_and_chart_contracts() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let windowing = fs::read_to_string(root.join("window").join("windowing.rs")).unwrap();
    let detached = fs::read_to_string(root.join("window").join("detached.rs")).unwrap();
    let chart_tabs_mod = fs::read_to_string(root.join("chart_tabs").join("mod.rs")).unwrap();
    let chart_tabs_windows =
        fs::read_to_string(root.join("chart_tabs").join("windows.rs")).unwrap();
    let chart_detached_host = fs::read_to_string(
        root.join("chart_tabs")
            .join("detached_host")
            .join("render.rs"),
    )
    .unwrap();

    assert!(
        windowing.contains("fn detached_panel_window_options(")
            && windowing.contains("fn detached_chart_window_options(")
            && !windowing.contains("fn detached_window_options("),
        "windowing.rs must expose separate detached panel/chart factories, not one ambiguous detached_window_options"
    );
    assert!(
        windowing
            .contains("owned_window_options(title, window_bounds, display_id, None, owner, true)"),
        "detached panel windows must keep owner-aware owned-window semantics"
    );
    assert!(
        windowing.contains("options.taskbar_visibility = WindowTaskbarVisibility::Hidden"),
        "detached chart windows must explicitly hide taskbar entries while staying independent"
    );
    assert!(
        detached.contains("detached_panel_window_options("),
        "generic detached panels must use the owner-aware panel factory"
    );
    // The chart-window lifecycle spans two files: `windows.rs` picks the options factory,
    // `detached_host/render.rs` hides the taskbar entry once the window exists. The negatives
    // stay scoped to `windows.rs` — that is where an owner-carrying panel factory could appear.
    assert!(
        chart_tabs_windows.contains("detached_chart_window_options(")
            && chart_detached_host.contains("hide_window_from_taskbar(window)")
            && !chart_tabs_windows.contains("owner: Option<AnyWindowHandle>")
            && !chart_tabs_windows.contains("detached_panel_window_options("),
        "detached chart windows must use the independent chart factory and must not carry owner in the chart lifecycle"
    );
    assert!(
        !chart_tabs_mod.contains("window.window_handle(), cx")
            && !chart_tabs_mod.contains("Some(owner)"),
        "ChartTabs restore/detach must not pass owner into detached chart windows"
    );
}

#[test]
fn terminal_secondary_tool_windows_use_tool_window_options() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let settings = fs::read_to_string(root.join("settings").join("mod.rs")).unwrap();
    let strategies = fs::read_to_string(root.join("strategies").join("window.rs")).unwrap();
    let assets = fs::read_to_string(root.join("panels").join("assets").join("window.rs")).unwrap();

    assert!(
        settings.contains("tool_window_options(")
            && strategies.contains("tool_window_options(")
            && assets.contains("tool_window_options("),
        "settings, strategies and assets are MoonWindowFrame::tool windows and must use tool_window_options"
    );
    assert!(
        !settings.contains("standalone_window_options(")
            && !strategies.contains("standalone_window_options(")
            && !assets.contains("standalone_window_options("),
        "tool/secondary windows must not be opened as standalone taskbar applications"
    );
}

#[test]
fn terminal_windows_use_closed_window_frame_api() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);

    let mut violations = Vec::new();
    for path in sources {
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        let rel = path.strip_prefix(&root).unwrap_or(&path);
        let rel_text = rel.to_string_lossy().replace('\\', "/");
        for (line_ix, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            let is_windowing = rel_text == "window/windowing.rs";
            let is_design = rel_text == "design.rs";
            if trimmed.contains("MoonWindowChrome::new")
                || trimmed.contains("MoonWindowChromeButton")
                || trimmed.contains("WindowControlArea::Drag")
                || trimmed.contains("start_window_move")
                || trimmed.contains("titlebar_double_click")
                || (!is_design
                    && (trimmed.contains("logo_sized(")
                        || trimmed.contains("logo_mark(")
                        || trimmed.contains("design::logo_sized")
                        || trimmed.contains("design::logo_mark")))
                || (!is_windowing && trimmed.contains("WindowOptions {"))
            {
                violations.push(format!("{}:{}: {}", path.display(), line_ix + 1, trimmed));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "terminal windows must go through windowing.rs + MoonWindowFrame instead of ad-hoc chrome/window options:\n{}",
        violations.join("\n")
    );
}

#[test]
fn terminal_overlays_use_moonui_window_layers_and_moon_components() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let strategies_mod = fs::read_to_string(root.join("strategies").join("mod.rs")).unwrap();
    let strategies_tree =
        fs::read_to_string(root.join("strategies").join("tree").join("ui.rs")).unwrap();
    let strategies_dialogs =
        fs::read_to_string(root.join("strategies").join("tree").join("dialogs.rs")).unwrap();
    let strategies_menu =
        fs::read_to_string(root.join("strategies").join("tree").join("menu.rs")).unwrap();
    let strategies_params = fs::read_to_string(root.join("strategies").join("params.rs")).unwrap();
    let assets_mod = fs::read_to_string(root.join("panels").join("assets").join("mod.rs")).unwrap();
    let assets_wallets =
        fs::read_to_string(root.join("panels").join("assets").join("wallets.rs")).unwrap();

    assert!(
        assets_wallets.contains("WindowExt as _")
            && assets_wallets.contains("window.open_unique_moon_dialog(")
            && assets_wallets.contains(".close_button(true)")
            && !assets_mod.contains("self.transfer_dialog(")
            && !assets_wallets.contains("fn transfer_dialog("),
        "Assets transfer modal must use a unique MoonUI Root dialog with a visible close button, not a manual panel child overlay"
    );
    assert!(
        strategies_dialogs.contains("WindowExt as _")
            && strategies_dialogs.contains("window.open_unique_moon_dialog(")
            && strategies_dialogs.contains("fn op_has_close_button(")
            && !strategies_tree.contains("fn op_overlay(")
            && !strategies_mod.contains("op_overlay(cx)")
            && !strategies_mod.contains("popup_overlay(cx)")
            && !strategies_params.contains("fn popup_overlay("),
        "Strategies modal overlays must use unique MoonUI Root dialogs with close-button policy, not manual absolute overlays"
    );
    assert!(
        strategies_menu.contains("MoonContextMenuWindowExt")
            && strategies_menu.contains("window.open_moon_context_menu(")
            && !strategies_mod.contains("menu: Option<tree::ui::ContextMenu>")
            && !strategies_mod.contains("menu_overlay(cx)")
            && !strategies_tree.contains("fn menu_overlay(")
            && !strategies_tree.contains("let mut list = v_flex()")
            && !strategies_tree.contains(".child(list)"),
        "Strategies context menu must use the MoonUI Root-owned context menu layer, not a panel child overlay"
    );
}

#[test]
fn firetest_chart_smoke_stays_runtime_behavior_scenario() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    // FireTest is a directory of stage modules, not one file: the invariants below are about the
    // scenario as a whole, so they read every source under it rather than a single path that a
    // later split would silently empty out.
    let firetest_dir = root.join("firetest");
    let mut firetest_files = Vec::new();
    rust_sources(&firetest_dir, &mut firetest_files);
    // Name the modules that carry the run's shape. A count would pass on any two files; these are
    // the ones the invariants below are actually about — the plan, the dispatcher, the scoring, and
    // the per-stage directory.
    for required in ["mod.rs", "plan.rs", "verdict.rs", "stages/mod.rs"] {
        let path = firetest_dir.join(required);
        assert!(
            firetest_files.contains(&path),
            "FireTest must keep {required} under src/firetest/: the scenario's plan, dispatch and \
             scoring each stay a module of their own"
        );
    }
    // COMMENTS ARE STRIPPED, and that is the point. Concatenating a directory sweeps in every
    // `//!` and `///` line, and a rule about what the code DOES must not be satisfiable — or
    // breakable — by prose that merely mentions the thing. `lines()` also drops the `\r` of a CRLF
    // checkout, which is why no separate normalization pass is needed.
    let firetest = firetest_files
        .iter()
        .map(|path| fs::read_to_string(path).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let docs = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("docs")
            .join("FIRETEST.md"),
    )
    .unwrap();

    assert!(
        firetest.contains("Phase::WaitOpen")
            && firetest.contains("Phase::CommandErrorContract")
            && firetest.contains("fn verify_command_error_contract(")
            && firetest.contains("Phase::ToolWindowsOpen")
            && firetest.contains("Phase::ToolWindowsVerifyOpen")
            && firetest.contains("Phase::ToolWindowsDedup")
            && firetest.contains("Phase::ToolWindowsVerifyDedup")
            && firetest.contains("fn request_tool_windows_open(")
            && firetest.contains("fn verify_tool_windows_open(")
            && firetest.contains("fn verify_tool_windows_dedup(")
            && firetest.contains("Phase::RootOverlayContract")
            && firetest.contains("fn verify_root_overlay_contract(")
            && firetest.contains("Phase::PriceScale50")
            && firetest.contains("Phase::PriceScale20")
            && firetest.contains("Phase::PriceScaleAuto")
            && firetest.contains("fn verify_price_scale(")
            && firetest.contains("fn try_open_chart(")
            && firetest.contains("fn start_mouse_storm(")
            && firetest.contains("fn evaluate_and_exit(")
            && firetest.contains("record_diag_sample(")
            && firetest.contains("observe_chart_probe("),
        "FireTest chart-smoke must remain a runtime behavior scenario: open real chart, observe probe, send native mouse input, evaluate metrics"
    );
    assert!(
        !firetest.contains("include_str!(")
            && !firetest.contains("fs::read_to_string")
            && !firetest.contains("run_ui_overlay_contract")
            && !firetest.contains("PRE_CHART_TESTS"),
        "FireTest не должен читать исходники; статические архитектурные проверки живут в tests/theme_contract.rs"
    );
    assert!(
        firetest.contains("\"chart-smoke\" => Script::ChartSmoke")
            && !firetest.contains("\"ui-overlay\"")
            && !firetest.contains("\"overlay-contract\"")
            && !firetest.contains("\"text-smoke\""),
        "new UI/chart checks must be added to chart-smoke stages, not separate debug scripts"
    );
    assert!(
        docs.contains("находит реальные bounds графика")
            && docs.contains("stage=idle_floor")
            && docs.contains("stage=command_error_contract")
            && docs.contains("stage=tool_windows_open")
            && docs.contains("stage=tool_windows_verify_open")
            && docs.contains("stage=tool_windows_dedup")
            && docs.contains("stage=tool_windows_verify_dedup")
            && docs.contains("stage=root_overlay_contract")
            && docs.contains("stage=price_scale_50")
            && docs.contains("stage=price_scale_20")
            && docs.contains("stage=price_scale_auto")
            && docs.contains("настоящий оконный input path")
            && docs.contains("FireTest проверяет поведение и нагрузку")
            && !docs.contains("include_str!")
            && !docs.contains("source contract"),
        "docs/FIRETEST.md должен описывать FireTest как runtime/perf сценарий, а не статическую проверку исходников"
    );
}

#[test]
fn a_diagnostic_run_cannot_flush_the_debounced_workspace_state() {
    // FireTest drives the real app: it opens tool windows, switches the locale, changes the price
    // scale, and will detach and repin panels. Every one of those marks state dirty, and for a long
    // time the 100 ms tick duly flushed it — so running the diagnostic quietly rewrote the saved
    // workspace it was supposed to be observing.
    //
    // This covers the two DEBOUNCED workspace flushes only. It is deliberately not a claim that a
    // diagnostic run writes nothing — the report DB writer, `strat_db`, `AppConfig::load`'s uid
    // save and panels that write straight through all bypass the dirty-flag mechanism. What it
    // does pin is that both debounced flushes are gated as a WHOLE: a guard per `*_dirty` branch
    // is one that a newly persisted thing can be added without.
    let startup = read_src("startup.rs");
    let main_rs = read_src("main.rs");

    assert!(
        main_rs.contains("persist_allowed: bool"),
        "Backend must carry the flag that decides whether this process may persist at all"
    );
    assert!(
        startup.contains("persist_allowed: firetest_config.is_none()"),
        "the flag must be derived from --debug-script at construction, not set later by a caller \
         who might forget"
    );

    let quit = braced_body(&startup, "cx.on_app_quit(");
    assert!(
        quit.contains("if !b.persist_allowed {"),
        "the quit flush must bail out before it writes anything"
    );

    // The debounced flush is the one that actually fires during a run: FireTest ends with
    // `std::process::exit`, which never reaches `on_app_quit` at all.
    //
    // Scope, not presence. A guard wrapping a single `*_dirty` branch would satisfy a bare
    // `contains`, which is exactly the per-file form the rule above forbids — so the guarded block
    // is sliced out and every saver has to be found INSIDE it.
    let guarded = braced_body(&startup, "if b.persist_allowed {");
    for saver in [
        "b.layout.save()",
        "dock_persist::save_all",
        "detached::save_all",
        "chart_persist::save_all",
        "b.tab_badges.save()",
        "b.figures.borrow_mut().save()",
        "b.config.save()",
    ] {
        assert!(
            guarded.contains(saver),
            "{saver} must sit inside the `if b.persist_allowed` block: a debounced save outside it \
             writes the developer's workspace during a --debug-script run"
        );
    }
    // And the notify that follows the block must stay OUTSIDE it — suppressing persistence must
    // not also suppress the backend wake the rest of the app depends on.
    assert!(
        !guarded.contains("b.flush_backend_notify(cx)"),
        "the guard must cover persistence only, not the backend notify that follows it"
    );
}
