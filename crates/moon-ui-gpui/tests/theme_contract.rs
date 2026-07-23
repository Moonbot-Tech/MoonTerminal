use std::fs;
use std::path::{Path, PathBuf};

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|err| {
        panic!("failed to read {}: {err}", dir.display());
    });
    for entry in entries {
        let entry = entry.unwrap_or_else(|err| panic!("failed to read dir entry: {err}"));
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn terminal_ui_uses_runtime_moon_ui_theme() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);

    let mut violations = Vec::new();
    for path in sources {
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        for (line_ix, line) in text.lines().enumerate() {
            let check = line.replace("moon_ui::", "moon_ui__");
            if check.contains("MoonPalette::TERMINAL")
                || check.contains("moon_core::palette")
                || check.contains("use moon_core::palette")
                || check.contains("palette::")
            {
                violations.push(format!(
                    "{}:{}: {}",
                    path.display(),
                    line_ix + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "terminal UI must use MoonPalette::active/MoonTheme runtime config, not old palette sources:\n{}",
        violations.join("\n")
    );
}

#[test]
fn chart_background_policy_keeps_gpu_canvas_under_scene() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut chartdx_sources = Vec::new();
    rust_sources(&root.join("chartdx"), &mut chartdx_sources);
    let chartdx = chartdx_sources
        .iter()
        .map(|path| {
            fs::read_to_string(path)
                .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let chart_panel = [
        root.join("panels").join("chart").join("mod.rs"),
        root.join("panels").join("chart").join("render.rs"),
    ]
    .into_iter()
    .map(|path| {
        fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
    })
    .collect::<Vec<_>>()
    .join("\n");
    let chart_tabs_mod = fs::read_to_string(root.join("chart_tabs").join("mod.rs")).unwrap();
    let chart_tabs_windows =
        fs::read_to_string(root.join("chart_tabs").join("windows.rs")).unwrap();
    let chart_tabs = format!("{chart_tabs_mod}\n{chart_tabs_windows}");
    // The shell is a module tree, not one file: the DockArea is built in `init.rs` while the
    // body is painted in `render.rs`. The policy assertions below must grep the whole set.
    let shell = ["mod.rs", "init.rs", "render.rs"]
        .into_iter()
        .map(|name| {
            let path = root.join("shell").join(name);
            fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let detached = fs::read_to_string(root.join("detached.rs")).unwrap();

    assert!(
        chartdx.contains("gpui::gpu_canvas(self.canvas.clone())")
            && !chartdx.contains("add_gpu_pass")
            && !chart_panel.contains("request_continuous_presentation"),
        "chart must use element-scoped gpu_canvas, not old window-global pass/continuous present"
    );
    assert!(
        chart_panel.contains("fn background_policy(&self, _cx: &App) -> MoonBackgroundPolicy")
            && chart_panel.contains("MoonBackgroundPolicy::NoFill"),
        "ChartPanel must keep NoFill background policy"
    );
    assert!(
        chart_tabs.contains("fn background_policy(&self, _cx: &App) -> MoonBackgroundPolicy")
            && chart_tabs.contains("MoonBackgroundPolicy::NoFill"),
        "ChartTabs host must keep NoFill background policy"
    );
    assert!(
        shell.contains(".background_policy(MoonBackgroundPolicy::NoFill)")
            && shell.contains(".tab_background_policy(MoonBackgroundPolicy::NoFill)"),
        "main shell DockArea path must keep NoFill policies"
    );
    assert!(
        chart_tabs_windows.contains(
            "Root::new(host, window, cx).background_policy(MoonBackgroundPolicy::NoFill)"
        ),
        "detached/debug chart windows must keep NoFill roots so UnderScene gpu_canvas stays visible"
    );
    assert!(
        !shell.contains(".bg(rgb(p.shell))\n                    .child(self.panel.clone())")
            && !chart_tabs
                .contains(".bg(rgb(p.shell))\n                    .child(self.panel.clone())"),
        "chart window body must not paint an opaque GPUI quad over UnderScene gpu_canvas"
    );
    assert!(
        detached.contains(".background_policy(MoonBackgroundPolicy::Opaque)"),
        "detached non-chart windows must paint an explicit opaque root"
    );
}

#[test]
fn main_chart_stack_rmb_toggle_uses_full_chart_area_not_plot_only() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let main_stack = fs::read_to_string(root.join("chart_tabs").join("main_stack.rs")).unwrap();

    assert!(
        main_stack.contains("window_pos_allows_main_stack_toggle(event.position)"),
        "Main stack RMB fullscreen/stack toggle must use the full chart panel hit-test, including orderbook glass"
    );
    assert!(
        !main_stack.contains("window_pos_in_chart_plot(event.position)"),
        "Main stack RMB fullscreen/stack toggle must not regress to plot-only hit-test"
    );
}

#[test]
fn terminal_windowing_separates_detached_panel_and_chart_contracts() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let windowing = fs::read_to_string(root.join("windowing.rs")).unwrap();
    let detached = fs::read_to_string(root.join("detached.rs")).unwrap();
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
    let strategies = fs::read_to_string(root.join("strategies").join("mod.rs")).unwrap();
    let assets = fs::read_to_string(root.join("panels").join("assets").join("mod.rs")).unwrap();

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
            let is_windowing = rel_text == "windowing.rs";
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
    let strategies_tree = fs::read_to_string(root.join("strategies").join("tree_ui.rs")).unwrap();
    let strategies_dialogs =
        fs::read_to_string(root.join("strategies").join("tree_dialogs.rs")).unwrap();
    let strategies_menu = fs::read_to_string(root.join("strategies").join("tree_menu.rs")).unwrap();
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
            && !strategies_mod.contains("menu: Option<tree_ui::ContextMenu>")
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
    let firetest = fs::read_to_string(root.join("firetest.rs")).unwrap();
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

/// Read one production source file for the static bans below, with line endings normalized to LF.
///
/// The normalization is load-bearing, not tidiness: these checkouts carry CRLF on disk, so any
/// pattern written with a bare `\n` — see [`fn_body`]'s closing-brace delimiter — silently matches
/// nothing and hands back a span far larger than intended. A ban that quietly stops bounding
/// anything still reports PASS.
fn read_src(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(rel);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
    text.replace("\r\n", "\n")
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
    // `clock_parts` and reimplement the strings or the timezone-visibility rule for itself, which
    // is exactly the drift that puts the popup off its trigger, and a name-only check stays green
    // through it.
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

/// The body of a top-level `fn`, from its signature to the closing brace in column 0.
///
/// Deliberately excludes the doc comment above the signature: a rule about what a function CALLS
/// must not be satisfiable by prose that merely mentions the callee.
fn fn_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let after = source
        .split_once(signature)
        .unwrap_or_else(|| panic!("expected to find `{signature}` in the source"))
        .1;
    after.split("\n}\n").next().unwrap_or(after)
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

/// A closure handed to `MoonTree` must never capture a STRONG handle to the view that owns the
/// tree state.
///
/// `MoonTree` moves both the row renderer and the row decorators into the long-lived
/// `MoonTreeState` on every frame (MoonUI `tree.rs`, `impl RenderOnce for Tree`). When that state
/// is a field of the same view, a strong capture closes the cycle
/// `View -> tree_state -> closure -> View`: the view never drops, so its `on_release` never runs
/// and its subscriptions are never released.
///
/// This already shipped once as a user-visible bug — the Settings window silently refused to
/// reopen, because its `on_release` was what cleared the draft that `settings::open` gates on.
/// `strategies/tree_moon.rs` has the same shape with a different symptom: `strategies::open` has
/// no such gate, so it leaks a view plus its subscriptions on every open/close cycle instead.
///
/// The plausible edit this catches: someone reads the `upgrade()` dance as ceremony and
/// "simplifies" it back to `cx.entity()`, which compiles and behaves correctly for one session.
#[test]
fn moon_tree_closures_hold_weak_view_handles() {
    let text = read_src("strategies/tree_moon.rs");

    assert!(
        text.contains("cx.entity().downgrade()"),
        "tree_moon.rs must downgrade the view handle before handing closures to MoonTree"
    );
    assert!(
        !text.contains("let view = cx.entity();"),
        "tree_moon.rs captures a STRONG view handle: MoonTree retains its renderer and decorators \
         in MoonTreeState, which this view owns, so the view can never drop"
    );
}

/// The Core-status panel renders protocol v4 `Event::KernelHealth` telemetry.
/// Process vs system CPU is a SCOPE distinction, not a time average: a future
/// edit that re-adds a machine-wide-CPU column under an "average" label, or
/// resurrects the memory columns v4 has no source for, reddens here.
#[test]
fn core_status_table_binds_scoped_telemetry_columns() {
    let table = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("panels")
        .join("core_status")
        .join("table.rs");
    let text = fs::read_to_string(&table)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", table.display()));

    for key in [
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
}
