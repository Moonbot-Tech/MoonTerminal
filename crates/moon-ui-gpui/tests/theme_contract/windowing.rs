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
    let chart_detached_host =
        fs::read_to_string(root.join("chart_tabs").join("detached_host").join("mod.rs")).unwrap();

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
    // Scoped to the chart factory's own body: the independent Profit Monitor factory now sets the
    // same `Hidden` literal for the same reason, so a file-level `contains` no longer distinguishes
    // the two and would stay green even if only the chart factory regressed to `Visible`.
    let detached_chart_factory =
        code_only(braced_body(&windowing, "fn detached_chart_window_options("));
    assert!(
        detached_chart_factory
            .contains("options.taskbar_visibility = WindowTaskbarVisibility::Hidden"),
        "detached chart windows must explicitly hide taskbar entries while staying independent"
    );
    assert!(
        detached.contains("detached_panel_window_options("),
        "generic detached panels must use the owner-aware panel factory"
    );
    // The chart-window lifecycle spans two files: `windows.rs` picks the options factory,
    // `detached_host/mod.rs` arms the shared taskbar-hide burst once the window exists. The
    // negatives stay scoped to `windows.rs` — that is where an owner-carrying panel factory could
    // appear. Comments are stripped from the host body: its own doc comment names the helper by
    // prose, which must not satisfy a check about actually calling it.
    assert!(
        chart_tabs_windows.contains("detached_chart_window_options(")
            && code_only(&chart_detached_host).contains("hide_window_from_taskbar_soon(window)")
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
    let core_expert = fs::read_to_string(root.join("core_expert.rs")).unwrap();

    assert!(
        settings.contains("tool_window_options(")
            && strategies.contains("tool_window_options(")
            && assets.contains("tool_window_options(")
            && core_expert.contains("tool_window_options("),
        "settings, strategies, assets and the expert core-settings window are \
         MoonWindowFrame::tool windows and must use tool_window_options"
    );
    assert!(
        !settings.contains("standalone_window_options(")
            && !strategies.contains("standalone_window_options(")
            && !assets.contains("standalone_window_options(")
            && !core_expert.contains("standalone_window_options("),
        "tool/secondary windows must not be opened as standalone taskbar applications"
    );
}

/// `windowing.rs:profit_monitor_window_options` must remain independent but carry no taskbar
/// button of its own — the terminal shows exactly one taskbar icon. Flipping `taskbar_visibility`
/// back to `Visible` reopens the second taskbar icon this factory exists to suppress; dropping the
/// independent relationship instead makes the desktop monitor minimize with a Main window or
/// disappear without a restore route.
#[test]
fn profit_monitor_is_independent_but_carries_no_taskbar_button() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let windowing = fs::read_to_string(root.join("window").join("windowing.rs")).unwrap();
    let monitor = read_module("analytics/profit_monitor");
    let startup = read_startup();
    let factory = code_only(braced_body(&windowing, "fn profit_monitor_window_options("));

    assert!(factory.contains("options.relationship = WindowRelationship::default()"));
    assert!(factory.contains("WindowTaskbarVisibility::Hidden"));
    assert!(!factory.contains("WindowTaskbarVisibility::Visible"));
    assert!(monitor.contains("profit_monitor_window_options("));
    assert!(monitor.contains("MoonWindowFrame::tool("));
    assert!(!startup.contains("group_windows.insert(\"profit_monitor\""));
}

/// Both independent-window classes — the Profit Monitor and detached charts — arm the taskbar-hide
/// burst at open AND re-arm it from `cx.observe_window_activation`. `DeleteTab` is not durable
/// window state, and the shell republishes the taskbar item whenever an iconic window is restored
/// (`windowing.rs::hide_window_from_taskbar_soon`'s own doc explains why). A future author who
/// deletes either re-arm, assuming the open-time burst alone is enough, brings the second
/// MoonTerminal taskbar icon back permanently the first time the user minimizes and restores that
/// window.
#[test]
fn profit_monitor_and_detached_chart_windows_rearm_taskbar_hide_on_activation() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let monitor = read_module("analytics/profit_monitor");
    let chart_detached_host =
        fs::read_to_string(root.join("chart_tabs").join("detached_host").join("mod.rs")).unwrap();

    // Comments stripped: both constructors document the re-arm by naming the exact helper and
    // `observe_window_activation`, which must not itself satisfy a check about calling them. The
    // burst must be armed once directly AND again from inside the activation observer — a bare
    // `contains` cannot tell "armed twice" from "armed once and merely mentioned twice in prose",
    // so this counts real call sites.
    let monitor_ctor = code_only(braced_body(
        &monitor,
        "fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {",
    ));
    let monitor_rearms = monitor_ctor
        .matches("hide_window_from_taskbar_soon(window)")
        .count();
    assert!(
        monitor_rearms >= 2
            && monitor_ctor.contains("cx.observe_window_activation(window,")
            && monitor_ctor.contains("this.taskbar_hide =")
            && monitor_ctor.contains("this.taskbar_hide.cancel();"),
        "ProfitMonitorView::new must arm the taskbar-hide burst at open AND re-arm it from \
         cx.observe_window_activation — the open-time burst alone cannot survive the shell \
         republishing the taskbar item when the window is restored from minimized"
    );

    let host_ctor = code_only(braced_body(&chart_detached_host, "pub(super) fn new("));
    let host_rearms = host_ctor
        .matches("hide_window_from_taskbar_soon(window)")
        .count();
    let host_activation = host_ctor
        .split("cx.observe_window_activation(window,")
        .nth(1)
        .and_then(|tail| tail.split(".detach();").next())
        .expect("detached chart must retain a native activation observer");
    let host_cancel = host_activation
        .find("this.taskbar_hide.cancel();")
        .expect("activation must cancel the previous taskbar burst");
    let host_rearm = host_activation
        .find("this.taskbar_hide = crate::window::windowing::hide_window_from_taskbar_soon(window)")
        .expect("activation must arm one replacement taskbar burst");
    assert!(
        host_rearms >= 2 && host_cancel < host_rearm,
        "DetachedChartHost::new must arm the taskbar-hide burst at open AND re-arm it from \
         cx.observe_window_activation for the same reason as the Profit Monitor"
    );
}

/// `profit_monitor/window.rs::open_window` must yield through a real timer before native creation and hold
/// one pending authority until the result is published; replacing the timer with `defer`, dropping
/// the pending guard, or clearing any singleton from an old release makes this assertion red and
/// restores the reported UI stall or duplicate/stale monitor lifecycle.
#[test]
fn profit_monitor_creation_and_release_are_deferred_and_identity_safe() {
    let monitor = code_only(&read_module("analytics/profit_monitor"));
    let monitor_window = code_only(&read_src("analytics/profit_monitor/window.rs"));
    let windowing = code_only(&read_src("window/windowing.rs"));
    let open = braced_body(&monitor_window, "fn open_window(");
    let explicit_open = braced_body(&monitor_window, "pub(crate) fn open(");
    let restore = braced_body(&monitor_window, "pub(crate) fn restore(");
    let constructor = braced_body(
        &monitor,
        "fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {",
    );
    let taskbar_worker = braced_body(&windowing, "fn run_taskbar_hide_worker(");

    assert!(
        open.contains("backend.profit_monitor_open_pending.as_mut()")
            && open.contains("request.upgrade(activate)")
            && open.contains("Some(ProfitMonitorOpenRequest::new(activate))")
            && open.contains("executor.timer(std::time::Duration::from_millis(1)).await")
            && open.contains("if backend.read(cx).quitting")
            && open.contains("cx.open_window(options")
            && open.find("executor.timer(std::time::Duration::from_millis(1)).await")
                < open.find("if backend.read(cx).quitting")
            && open.find("if backend.read(cx).quitting") < open.find("cx.open_window(options")
            && open
                .matches("backend.profit_monitor_open_pending = None")
                .count()
                >= 2,
        "Profit Monitor creation must remain single-flight, yield, and refuse native creation once shutdown begins"
    );
    assert!(
        constructor.contains("handle.window_id() != this.window_id")
            && constructor.contains("backend.profit_monitor_window = None")
            && constructor.contains("this.taskbar_hide.cancel();"),
        "an old native release must cancel its taskbar worker and clear only its exact singleton"
    );
    assert!(
        explicit_open.contains("open_window(backend, owner, owner_display, true, cx)")
            && restore.contains("open_window(backend, owner, None, false, cx)")
            && open.contains("let alive = if activate")
            && open.contains("let alive = if request.activate")
            && open.contains("if request.activate")
            && open.contains("activate_new_window(handle.into(), cx)"),
        "startup restore must stay non-activating while an explicit open upgrades and activates the one pending request"
    );
    assert!(
        taskbar_worker.contains("CoInitializeEx(None, COINIT_APARTMENTTHREADED)")
            && taskbar_worker.contains("CoCreateInstance(&TaskbarList")
            && taskbar_worker.contains("taskbar.DeleteTab(hwnd)")
            && !taskbar_worker.contains("cx.update")
            && !taskbar_worker.contains(".await"),
        "taskbar retries must create, use, and drop COM inside their background apartment without returning to GPUI"
    );
}

/// Decorative animation goes through `crate::pulse`, never `with_animation`.
///
/// GPUI drives `with_animation` from `request_animation_frame`, which notifies the OWNER VIEW and
/// marks every ancestor dirty. So an animation re-renders its owning view in full at vblank for
/// its whole duration. MoonUI's dock caches each panel, which spares SIBLING panels — it does not
/// spare the owner or anything the owner renders. On a chart stack that measured 274-338 chart
/// renders/s from one arriving chart, against 5-6/s for the same window under a mouse storm.
///
/// The replacement is a ~10 Hz timer plus an opacity read from the owner's own `Instant`. This ban
/// exists because the defect is invisible in review: the call site looks like an ordinary builder
/// method, and the cost lands on unrelated views in the same window.
#[test]
fn decorative_animation_goes_through_the_pulse_timer() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);

    let mut violations = Vec::new();
    for path in sources {
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        let rel = path.strip_prefix(&root).unwrap_or(&path);
        for (line_ix, line) in text.lines().enumerate() {
            // Comment lines are skipped on purpose: `pulse.rs` and both call sites explain the ban
            // by naming the banned call, and prose must not trip its own rule.
            if line.trim_start().starts_with("//") {
                continue;
            }
            // Both halves of the API: the builder method and the descriptor it takes. Banning only
            // the method leaves `AnimationExt::with_animation(el, ..)` and any future spelling that
            // still needs an `Animation` through.
            if line.contains("with_animation(") || line.contains("Animation::new(") {
                violations.push(format!("{}:{}", rel.display(), line_ix + 1));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "`with_animation` repaints every view in the window at vblank for its whole duration — \
         drive the pulse from `crate::pulse::arm` and read its opacity from `crate::pulse::phase` \
         instead: {violations:?}"
    );

    // The ban alone is satisfiable by deleting the decoration, and — worse — by keeping the
    // opacity while dropping the timer that advances it, which reads as "done" and leaves a
    // decoration frozen at whichever value the last unrelated repaint caught. So bind both halves
    // at every surviving call site: whoever draws a pulse must also arm a timer.
    // The chart stack is deliberately NOT paired here: its arrival flash moved into the chart's own
    // GPU pass, which is cheaper still. It gets its own binding below.
    {
        // Comment-stripped, like the ban above: prose naming the call must not satisfy a rule
        // about making it.
        let code = |rel: &str| {
            read_src(rel)
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        // News draws its tint from the shared helper; the Profit Monitor draws the same tint on a
        // table row. Both spell the drawing half `pulse::…tint(`, and both must arm the timer.
        for (owner, drawer, arm) in [
            ("panels/news/mod.rs", "panels/news/render.rs", "pulse::arm("),
            (
                "analytics/profit_monitor/mod.rs",
                "analytics/profit_monitor/line.rs",
                "pulse::arm_with(",
            ),
        ] {
            let drawing = code(drawer);
            assert!(
                drawing.contains("pulse::phase(") || drawing.contains("tint("),
                "{drawer} draws a pulse, so its opacity must come from `crate::pulse`"
            );
            let arming = code(owner);
            assert!(
                arming.contains(arm),
                "{owner} owns a pulse, so it must arm the repaint timer that advances it — an \
                 opacity with no timer freezes at whatever the last unrelated repaint left behind"
            );
        }

        // The chart's arrival flash: same two halves, different mechanism. It is drawn by the own
        // pass and paced there, and the load-bearing half is EXPIRY — leave `arrival_pulse` set and
        // that canvas requests a present ten times a second for the rest of the session, which
        // reads as a mysterious idle floor and no runtime gate would attribute it.
        let render_state = code("chartdx/render_state.rs");
        let frame = braced_body(&render_state, "fn frame(&mut self, info: GpuFrameInfo)");
        assert!(
            frame.contains("self.arrival_pulse = None"),
            "`RenderState::frame` must clear `arrival_pulse` when the flash is over — nothing else \
             stops the presents it requests"
        );
        assert!(
            frame.contains("CHART_ARRIVAL_PULSE"),
            "the arrival flash must count its presents, or a flash that never ends is invisible in \
             render_diag.log"
        );
        assert!(
            code("chart_tabs/add_stack.rs").contains("set_arrival_pulse("),
            "the chart stack must hand its arrival stamp to the panel's own pass; without it the \
             flash never starts and every test here still passes"
        );
    }
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
            // The ONE screen allowed to place a brand mark by hand: the main header, because
            // `MoonWindowFrame` draws MoonUI's own Moonbot lockup and the product ships the
            // MoonTerminal one. See `docs/WINDOWING.md`; everywhere else the frame still chooses.
            let is_main_header = rel_text == "chrome/terminal_chrome.rs";
            if trimmed.contains("MoonWindowChrome::new")
                || trimmed.contains("MoonWindowChromeButton")
                || trimmed.contains("WindowControlArea::Drag")
                || trimmed.contains("start_window_move")
                || trimmed.contains("titlebar_double_click")
                || (!is_design
                    && (trimmed.contains("logo_sized(")
                        || trimmed.contains("logo_mark(")
                        || trimmed.contains("design::logo_sized")
                        || trimmed.contains("design::logo_mark")
                        // Reaching for the asset by hand is the same violation as calling the
                        // helper, and the only way past a check that names helpers alone. A
                        // comment is free to NAME the folder — only code that opens a file in it
                        // is the violation, hence the trailing slash and the comment guard.
                        || (!trimmed.starts_with("//") && trimmed.contains("assets/brand/"))
                        || (!is_main_header && trimmed.contains("header_logo("))))
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
            && strategies_menu.contains("window.open_fitted_moon_context_menu(")
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

/// `startup.rs:dispatch_live_persistence` must remain inside the process-wide persistence gate;
/// moving it outside makes a FireTest run overwrite the developer's saved workspace.
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
    let startup = read_startup();
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
        "dispatch_live_persistence(b, &mut coord_persistence.borrow_mut())",
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
    assert!(
        !guarded.contains("b.layout.save()") && !guarded.contains("window_state_persist::save_all"),
        "the gated live GPUI loop may dispatch layout and Classic snapshots but must never write them synchronously"
    );
    // And the notify that follows the block must stay OUTSIDE it — suppressing persistence must
    // not also suppress the backend wake the rest of the app depends on.
    assert!(
        startup.contains("b.flush_backend_notify(cx)")
            && !guarded.contains("b.flush_backend_notify(cx)"),
        "the backend notify must exist and stay OUTSIDE the guard: suppressing persistence must \
         not also suppress the wake the rest of the app runs on"
    );
}

/// A secondary window opened from inside its owner's event handler must carry the display captured
/// at the call site, and an explicit detach must raise the window it just created.
///
/// Both halves are macOS defects that no Windows run can reproduce, which is exactly why they are
/// pinned statically. `saved_or_owner_display_id`'s owner fallback CANNOT resolve from within an
/// owner-window update — GPUI takes that window out of `cx.windows` for the duration — so a route
/// passing `None` there silently opens on the primary display. And a window created on another
/// display stays hidden until the next application activation, which is what made the reported
/// double-click "do nothing" until the user opened some other window.
///
/// Scoped to the function bodies that own each step, because both files legitimately contain a
/// SECOND `activate_new_window` — the branch that raises an already-detached panel — and a
/// file-level match would stay green with the post-spawn raise deleted.
#[test]
fn detach_routes_capture_the_owner_display_and_raise_what_they_open() {
    let docks = code_only(&read_src("shell/docks.rs"));
    let panels_common = code_only(&read_src("panels/common.rs"));
    let chart_windows = code_only(&read_src("chart_tabs/windows.rs"));
    let chart_tabs_mod = code_only(&read_src("chart_tabs/mod.rs"));
    let chart_strip = code_only(&read_src("chart_tabs/strip.rs"));

    let panel_routes = [
        ("dock tab", braced_body(&docks, "fn defer_detach_panel(")),
        (
            "panel toolbar",
            braced_body(&panels_common, "fn detach_button("),
        ),
    ];
    for (route, body) in panel_routes {
        assert!(
            body.contains("window_display_id(window, app)"),
            "the {route} detach route must capture the owner display at the call site: the owner              fallback cannot resolve while that window's slot is borrowed"
        );
        let spawn_at = body
            .find("detached::spawn(")
            .expect("every detach route must open through detached::spawn");
        // Bounded at the `match` arm's brace, not at the statement's `;`: the `Err` arm logs the
        // group and panel, and a slice reaching that far would accept the token from the log line.
        let spawn_args = body[spawn_at..]
            .split_once('{')
            .map(|(call, _)| call)
            .unwrap_or(&body[spawn_at..]);
        assert!(
            spawn_args.contains("owner_display"),
            "the {route} detach route must hand `spawn` the captured display, not a literal `None`"
        );
        let raise = body
            .rfind("activate_new_window(")
            .expect("every detach route must raise the window it opened");
        assert!(
            raise > spawn_at,
            "the {route} route must raise the window AFTER opening it, not only the pre-existing one"
        );
    }
    assert!(
        chart_strip.contains("window_display_id(window, app)")
            && chart_strip.contains("this.detach(tab_id, owner_display, cx)"),
        "the chart tab double-click — the reported gesture — must hand its own display to detach"
    );
    assert!(
        chart_tabs_mod.contains("window_display_id(window, cx)")
            && chart_tabs_mod.contains("this.restore_detached(owner_display, cx);"),
        "restored detached charts must receive the display of the window being built, because          `group_windows` is filled only after `open_window` returns"
    );
    let open_chart = braced_body(&chart_windows, "fn open_chart_window(");
    assert!(
        open_chart.contains("remembered.then_some(origin)"),
        "only a remembered origin may pick a display: the first-detach cascade point lies inside          the primary display and would answer for every monitor"
    );
    let chart_detach = braced_body(&chart_windows, "pub(super) fn detach(");
    assert!(
        !open_chart.contains("activate_new_window(")
            && chart_detach.contains("activate_new_window(window.into(), cx)")
            && chart_detach.find("upsert_spec") < chart_detach.find("activate_new_window("),
        "the chart window must be raised by the detach gesture AFTER its spec is recorded — never          inside the opener, which startup restoration also calls once per restored window"
    );
    assert!(
        code_only(&read_src("window/detached.rs")).contains("(!spec.cascade_origin).then_some("),
        "spawn must read the remembered-origin fact off the spec rather than re-deriving it from          a second file, which can disagree with the spec it describes"
    );
    assert!(
        docks.contains("self.defer_detach_panel(panel, false, cx);"),
        "the Backend-driven detach drain must not take the foreground: no gesture asked for it"
    );
    assert!(
        braced_body(&docks, "fn defer_detach_panel(").contains("if backend.read(app).quitting {"),
        "a detach deferred across shutdown must not create or raise a native window"
    );
}

/// Persisted window geometry must name its display, and that identity must be tried BEFORE the
/// coordinate route — never instead of it.
///
/// The order is the whole compatibility argument. Windows and X11 report global window coordinates,
/// so containment already resolves the right monitor there and has done so for every release; the
/// identity is added in front of it because it is the only answer macOS can give (its coordinates
/// are relative to the window's own screen, and every display reports a zero origin). Dropping the
/// containment pass would regress the platforms that work today; putting it first would leave a
/// resolvable identity unused after the monitors were rearranged.
#[test]
fn saved_geometry_names_its_display_and_identity_outranks_coordinates() {
    let windowing = code_only(&read_src("window/windowing.rs"));
    let resolve = braced_body(&windowing, "pub(crate) fn saved_or_owner_display_id(");
    let by_uuid = resolve
        .find("display_id_for_uuid(")
        .expect("a saved display identity must be resolved");
    let by_origin = resolve
        .find("WINDOW_COORDS_ARE_GLOBAL")
        .expect("the coordinate route must survive for platforms with global coordinates");
    let by_owner = resolve
        .find("owner_display.or_else(")
        .expect("the owner window must remain the final fallback");
    assert!(
        by_uuid < by_origin && by_origin < by_owner,
        "identity first, then coordinate containment, then the owner window"
    );

    // The platform gate is the whole cost story: both readers walk every monitor, and they sit in
    // `observe_window_bounds` callbacks that fire per step of a window drag. Off macOS the saved
    // coordinates already name the monitor, so that sweep would buy nothing — and removing either
    // gate silently puts a per-monitor Win32 enumeration on every WM_MOVE.
    for reader in ["fn window_display_uuid(", "fn display_identity("] {
        let body = braced_body(&windowing, reader);
        assert!(
            body.contains("if WINDOW_COORDS_ARE_GLOBAL {") && body.contains("return None;"),
            "{reader} must return early where coordinates already name the monitor, since reading the identity there costs a monitor sweep per drag event and buys nothing"
        );
    }

    // Every window that persists geometry must persist the display with it, through the one helper
    // that reads both — a rectangle saved without its monitor reopens on the wrong one.
    for module in [
        "analytics/mod.rs",
        "analytics/profit_monitor/mod.rs",
        "panels/assets/mod.rs",
        "panels/report/state.rs",
        "screener/view.rs",
        "settings/mod.rs",
        "strategies/state.rs",
        "window/detached.rs",
    ] {
        let source = code_only(&read_src(module));
        assert!(
            source.contains("window_geom_rect(window, cx)"),
            "{module} must capture geometry and display together"
        );
    }
}

/// `panels/chart/trade.rs` must keep every historical-order guard as the first executable
/// statement, and `panels/chart/render.rs` must keep both market-action routes behind
/// `!self.historical`. Moving one guard below its gesture resolution or removing either render
/// gate would let a closed-trade viewer issue live orders, or hide Panic Sell and Cancel Buy from
/// the main and detached live charts.
#[test]
fn historical_trade_windows_leave_no_live_order_or_market_action_route() {
    let trade = read_src("panels/chart/trade.rs");
    for (name, signature) in [
        ("place-order click", "pub(super) fn try_place_order_click("),
        ("move-orders click", "pub(super) fn try_move_orders_click("),
        ("place-order hotkey", "pub(crate) fn place_order_at_cursor("),
        (
            "cancel-order click",
            "pub(super) fn try_cancel_order_click(",
        ),
        ("order menu", "pub(super) fn try_open_order_menu("),
        ("cancel hovered order", "pub fn cancel_hovered_order("),
        ("send sells to zone", "pub(super) fn send_sells_to_zone("),
        ("split hovered order", "pub fn split_hovered_order("),
        ("start order drag", "pub(super) fn try_start_order_drag("),
    ] {
        let body = code_only(braced_body(&trade, signature));
        assert!(
            body.split_once('{')
                .expect("each order action must have a function body")
                .1
                .trim_start()
                .starts_with("if self.historical {"),
            "{name} must reject a historical chart before its first executable statement"
        );
    }

    let render = code_only(&read_src("panels/chart/render.rs"));
    assert!(
        render.contains("let market_actions = !self.historical;"),
        "market actions must be derived from the historical mode rather than hardcoded"
    );
    assert!(
        render.contains("if market_actions && !single_pane && !self.orderbook_only {"),
        "the multi-pane Cancel Buy and Panic Sell route must remain independently gated"
    );
    assert!(
        render.contains("let action_overlay = if market_actions && single_pane {"),
        "the single-pane Cancel Buy and Panic Sell overlay must remain independently gated"
    );

    let chart_mod = read_src("panels/chart/mod.rs");
    for signature in [
        "pub fn set_orderbook_enabled(",
        "pub fn set_action_btn_pos(",
    ] {
        assert!(
            code_only(braced_body(&chart_mod, signature)).contains("if self.historical"),
            "{signature} must refuse settings that could restore a historical chart's live UI"
        );
    }
    assert!(
        code_only(braced_body(&chart_mod, "pub fn new_historical("))
            .contains("sync_orderbook_refs("),
        "the historical constructor must drop its order-book subscription, not merely hide pixels"
    );

    let trade_window = code_only(&read_module("trade_window"));
    assert!(
        !trade_window.contains("set_orderbook_enabled(")
            && !trade_window.contains("set_action_btn_pos("),
        "the trade window must not directly re-enable the historical chart's live controls"
    );
    let opener = code_only(braced_body(
        &trade_window,
        "pub(crate) fn open_trade_window(",
    ));
    let pin = opener
        .find("view.tf_min = 1")
        .expect("the trade window must pin replay candles to one minute");
    let view = opener
        .find("let view = cx.new")
        .expect("the trade window must construct its view after configuring the panel");
    assert!(
        opener.contains("new_historical(") && opener.contains("set_candle_view(") && pin < view,
        "the opener must build a historical panel and pin one-minute candles before the first view fetch"
    );
}

/// Every window root must repair an empty focus on the way into its own frame.
///
/// GPUI dispatches a key event down the path of the FOCUSED node, and with the window blurred it
/// falls back to the dispatch tree's bare ROOT node, whose path holds no element listeners: the
/// root's `on_key_down` is skipped and EVERY hotkey dies until something focusable is clicked. The
/// UI stack blurs on two ordinary paths and hands the focus nowhere — `MoonPopover` closing with no
/// previous holder, and GPUI releasing a dropped focus handle — so a window that does not repair
/// this loses its keyboard for the rest of the session. Measured 2026-08-26, at the cost of most of
/// a day: four New Long presses in a row logged `window focus=NONE, dispatch depth=0` and did
/// nothing whatever, with no other symptom anywhere.
///
/// Asserted per window rather than once: the repair is worthless in the window that forgot it, and
/// the two roots are edited independently.
#[test]
fn every_window_root_restores_focus_when_nothing_holds_it() {
    for (name, path) in [
        ("group window", "shell/render.rs"),
        (
            "detached chart window",
            "chart_tabs/detached_host/render.rs",
        ),
        ("strategies window", "strategies/mod.rs"),
        ("trade window", "trade_window/render.rs"),
        ("analytics window", "analytics/render.rs"),
        ("profit monitor window", "analytics/profit_monitor/mod.rs"),
        ("report panel and its own window", "panels/report/render.rs"),
    ] {
        let raw = read_src(path);
        let src = code_only(&raw);
        assert!(
            src.contains("restore_root_focus(&self.focus, window, cx)"),
            "{name} ({path}) must call hotkeys::restore_root_focus from its render, or a blurred \
             window silently stops receiving every hotkey"
        );
    }
    // And the repair itself must stay conditional: taking focus unconditionally would pull it out
    // of whatever field the user is typing in, on every frame.
    let raw = read_src("hotkeys.rs");
    let hotkeys = code_only(&raw);
    let body = braced_body(&hotkeys, "pub fn restore_root_focus(");
    assert!(
        body.contains("window.focused(cx).is_none()"),
        "restore_root_focus must act only when NOTHING holds focus"
    );
}

/// Every way out of a coin search must hand the keyboard back.
///
/// The field keeps focus after a pick — nothing takes it away, and an input here is deliberately not
/// blurred just because something else was clicked. Visually the search is over; in fact every
/// keystroke still belongs to a text field, and a text field eats the editing shortcuts outright:
/// Ctrl+Z is Undo there and Ctrl+X is Cut, both perfectly ordinary keys to bind New Long and New
/// Short to. The hotkey then does nothing, with no symptom anywhere.
///
/// Listed per EXIT rather than checked once: the reason a user is finished with the field differs
/// by exit — picked a coin, clicked away, opened the selection in a new tab, chose a ticker, filtered
/// a report column — and an exit added later inherits the defect rather than the fix.
#[test]
fn every_coin_search_exit_releases_the_keyboard() {
    for (name, path, exits) in [
        (
            "chart tab strip and detached window",
            "chart_tabs/common.rs",
            // Pick, and the shared end-of-search funnel — the dismiss layer and a press on a
            // neighbouring toolbar control both run through `coin_toolbar_press_handler`, so they
            // are two exits behind one call. A third exit added here needs its own.
            2,
        ),
        ("open selection in a new tab", "chart_tabs/strip.rs", 1),
        ("report coin filter", "panels/report/render.rs", 2), // pick, dismiss
        ("header ticker picker", "shell/ticker.rs", 3),       // pick, hover-out, dismiss
    ] {
        let raw = read_src(path);
        let src = code_only(&raw);
        let found = src.matches("coin_search::release_focus(&").count();
        assert_eq!(
            found, exits,
            "{name} ({path}) must release focus on each of its {exits} coin-search exit(s), \
             found {found} — an exit that keeps focus silently disables every hotkey bound to an \
             editing shortcut"
        );
    }
}

/// Both chart toolbars arm the press that ends a market search on the groups flanking the field.
///
/// The market list is not a popover: it is a plain element with a dismiss layer under the toolbar
/// row, so the row itself is the one place that layer cannot reach — a dropdown trigger there stops
/// the press before it arrives. Coverage is therefore geometric, wired per section by hand, and
/// nothing about a fourth `chrome_section` or a control moved between them would fail to compile.
/// This is what says the wiring is still there, in both hosts, on BOTH sides of the field.
#[test]
fn both_chart_toolbars_end_the_search_on_a_neighbouring_press() {
    for (name, path) in [
        ("chart tab strip", "chart_tabs/strip.rs"),
        (
            "detached chart window",
            "chart_tabs/detached_host/render.rs",
        ),
    ] {
        let raw = read_src(path);
        let src = code_only(&raw);
        assert!(
            src.contains("coin_toolbar_press_handler(cx)"),
            "{name} ({path}) must build the end-of-search press handler"
        );
        let armed = src.matches("capture_any_mouse_down(end)").count();
        assert_eq!(
            armed, 2,
            "{name} ({path}) must arm that handler on BOTH control groups beside the market field, \
             found {armed} — an unarmed group leaves the list standing under whatever opens over it"
        );
    }
}

/// The ⚙ popup's own fields are enumerated in exactly one place.
///
/// Its inputs belong to the HOST, so one left focused when the popup stops rendering keeps
/// resolving: the window reads as focused while the dispatch path has already collapsed, and every
/// hotkey dies with nothing anywhere to say so (`hotkeys::restore_root_focus` documents why it
/// cannot repair that one). `release_layout_field_focus` walks `layout_fields()` to prevent it, so
/// a sixth field reaching the popup and not that list would silently restore the defect.
#[test]
fn layout_popup_field_list_covers_every_input() {
    let raw = read_src("chart_tabs/common.rs");
    let src = code_only(&raw);
    // Scoped to the ⚙ popup's own trait: `CoinPopupHost` lives in the same file and would otherwise
    // have its field getters counted as inputs this popup owes a release to.
    let trait_body = braced_body(&src, "trait LayoutPopupHost");
    let getters: Vec<&str> = trait_body
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("fn ")?;
            let (name, tail) = rest.split_once('(')?;
            (name.ends_with("_input") && tail.contains("-> &Entity<MoonInputState>"))
                .then_some(name)
        })
        .collect();
    assert!(
        getters.len() >= 5,
        "expected the ⚙ popup's input getters on LayoutPopupHost, found {getters:?}"
    );
    let list = braced_body(&src, "fn layout_fields(");
    for getter in &getters {
        assert!(
            list.contains(&format!("self.{getter}()")),
            "layout_fields() omits {getter}() — a field left out keeps the keyboard when the popup \
             closes and silently kills every hotkey"
        );
    }
    // And the release has to sit on the ONE close funnel. `MoonPopover` reports a close only when
    // it decided one, so the ✕ inside the popup — a plain flag flip — reaches no report at all;
    // hanging the release off the report alone silently leaves that exit stranding the focus.
    let funnel = braced_body(&src, "fn close_layout_popup(");
    assert!(
        funnel.contains("release_layout_field_focus("),
        "close_layout_popup must release the popup's own fields as it closes"
    );
    let direct = src.matches("close_chart_popup(ChartPopup::Layout").count();
    assert_eq!(
        direct, 1,
        "the ⚙ popup must be closed only through close_layout_popup, found {direct} direct \
         close_chart_popup(Layout) call(s) — a close that skips the funnel keeps the keyboard on a \
         field that has stopped rendering"
    );
    // `close_chart_popup` is a trait method, so either host could reach past the funnel from its
    // own module; the one call above is only "the funnel is the only one HERE".
    for host in [
        "chart_tabs/mod.rs",
        "chart_tabs/custom.rs",
        "chart_tabs/detached_host/mod.rs",
        "chart_tabs/settings.rs",
    ] {
        let host_src = code_only(&read_src(host));
        assert!(
            !host_src.contains("close_chart_popup(ChartPopup::Layout"),
            "{host} closes the ⚙ popup directly — it must go through close_layout_popup so the \
             popup's own fields hand the keyboard back"
        );
    }
}

/// A caption edit made INSIDE the trade window must be stored without separating the tab kinds.
///
/// `set_chart_labels_default` is the ⧉ press: besides storing the value it freezes the kinds it is
/// not addressing at what they currently show, and says so in its own hint. A right-click toggle in
/// one window is a statement about that window alone, so it goes through `store_chart_labels` —
/// which does the storing and nothing else. Routing it back through the press would perform the
/// kind-separation silently, on a gesture that never mentions it.
#[test]
fn a_trade_window_caption_edit_stores_without_separating_the_kinds() {
    let src = code_only(&read_src("trade_window/mod.rs"));
    let drain = braced_body(&src, "fn drain_panel_labels(");
    assert!(
        drain.contains("store_chart_labels("),
        "the window must store its own kind's captions through store_chart_labels"
    );
    assert!(
        !drain.contains("set_chart_labels_default("),
        "storing one window's captions must not perform the ⧉ press's kind separation"
    );
}
