//! Static font-family contracts for data surfaces and paired width measurements.
//!
//! `moon-ui-gpui` is a binary crate, so these tests read render sources rather than importing
//! views. Each assertion names a data surface whose values users compare across rows or read as
//! figures; changing one to the UI family would make a font sweep silently corrupt that visual
//! distinction.

use super::support::{braced_body, chain_between, code_only, read_src};

/// Return one render-root body without permitting comments to satisfy a source contract.
fn render_root(source: &str, render_impl: &str) -> String {
    let render_impl = braced_body(source, render_impl);
    code_only(braced_body(render_impl, "fn render("))
}

/// The panel, window, and strategy-pane roots below contain data tables, figures, names, or logs.
///
/// Breakage: changing any listed render root from `design::mono()` to `design::ui_font()` makes
/// values and row comparisons proportional, so users can no longer scan aligned data reliably.
#[test]
fn data_render_roots_keep_the_mono_family() {
    for (path, render_impl, surface) in [
        (
            "shell/render.rs",
            "impl Render for Shell",
            "the Shell trade and status data",
        ),
        (
            "panels/log/view.rs",
            "impl Render for LogPanel",
            "log lines",
        ),
        (
            "panels/orders/render.rs",
            "impl Render for OrdersPanel",
            "order-table values",
        ),
        (
            "panels/report/render.rs",
            "impl Render for ReportPanel",
            "report rows",
        ),
        (
            "panels/report/trade_log/view.rs",
            "impl Render for TradeLog",
            "trade-log lines",
        ),
        (
            "panels/assets/render.rs",
            "impl Render for AssetsView",
            "asset figures",
        ),
        (
            "panels/core_status/mod.rs",
            "impl Render for CoreStatusView",
            "core-status figures",
        ),
        (
            "panels/alerts/mod.rs",
            "impl Render for AlertsPanel",
            "alert rows",
        ),
        (
            "panels/news/mod.rs",
            "impl Render for NewsView",
            "news timestamps and tickers",
        ),
        (
            "analytics/render.rs",
            "impl Render for AnalyticsView",
            "analytics figures",
        ),
        (
            "analytics/profit_monitor/mod.rs",
            "impl Render for ProfitMonitorView",
            "profit-monitor rows",
        ),
        (
            "screener/view.rs",
            "impl Render for ScreenerView",
            "screener rows",
        ),
        (
            "strategies/mod.rs",
            "impl Render for StrategiesView",
            "strategy rows and names",
        ),
    ] {
        let source = read_src(path);
        let root = render_root(&source, render_impl);
        let mono = if path == "panels/log/view.rs" {
            ".font_family(crate::design::mono())"
        } else {
            ".font_family(design::mono())"
        };
        assert!(
            root.contains(mono),
            "{path}:{render_impl} must keep mono for {surface}; a proportional root makes compared values drift"
        );
    }

    for (path, outer, function, surface) in [
        (
            "strategies/tree/mod.rs",
            "impl StrategiesView",
            "pub(super) fn tree_panel(",
            "strategy tree names and row values",
        ),
        (
            "strategies/params.rs",
            "impl StrategiesView",
            "pub(super) fn params_panel(",
            "strategy parameter values",
        ),
        (
            "strategies/versions.rs",
            "impl StrategiesView",
            "pub(super) fn versions_panel(",
            "strategy-version timestamps and values",
        ),
    ] {
        let source = read_src(path);
        let outer = braced_body(&source, outer);
        let body = code_only(braced_body(outer, function));
        assert!(
            body.contains(".font_family(design::mono())"),
            "{path}:{function} must keep mono for {surface}; a proportional pane breaks comparison scanning"
        );
    }
}

/// Each independently placed crowd board must carry its own mono family in both layouts.
/// Reverting one branch to UI text must fail even when its sibling boards still use mono.
#[test]
fn independently_placed_crowd_boards_keep_the_mono_family() {
    let source = read_src("crowd.rs");
    let boards = braced_body(&source, "pub(crate) fn boards(");
    for branch in [
        "if this.parts.minute",
        "if this.parts.coins",
        "if this.parts.traders",
    ] {
        assert!(
            code_only(braced_body(boards, branch)).contains(".font_family(design::mono())"),
            "crowd.rs:{branch} must set mono on its independent data root"
        );
    }
}

/// Chart glyphs, chart-tab names, and header figures are data, not prose.
///
/// Breakage: replacing their mono face with the UI face changes the width of prices, axes, coin
/// names, timestamps, tickers, and balances while the adjacent geometry remains data-oriented.
#[test]
fn chart_and_header_data_text_keep_the_mono_family() {
    let text = read_src("chartdx/text/mod.rs");
    for function in [
        "fn draw_text_run(",
        "fn measure_text_run(",
        "fn draw_label_text_run(",
        "fn measure_label_text_run(",
        "fn measure_run_width(",
    ] {
        assert!(
            code_only(braced_body(&text, function)).contains("gpui::font(crate::design::mono())"),
            "chartdx/text/mod.rs:{function} must keep mono because chart axis and order-line text are compared figures"
        );
    }
    let captions = read_src("chartdx/text/captions.rs");
    for function in ["fn measure_caption_run(", "fn draw_caption_run("] {
        assert!(
            code_only(braced_body(&captions, function))
                .contains("gpui::font(crate::design::mono())"),
            "chartdx/text/captions.rs:{function} must keep mono because chart captions carry figures and labels with aligned geometry"
        );
    }
    let runs = read_src("chartdx/text/runs.rs");
    assert!(
        code_only(braced_body(&runs, "impl RenderState"))
            .contains("gpui::font(crate::design::mono())"),
        "chartdx/text/runs.rs:RenderState must keep mono for retained chart-text runs"
    );
    let stack = read_src("chart_tabs/stack.rs");
    assert_eq!(
        code_only(braced_body(&stack, "pub(super) fn chart_stack_card("))
            .matches(".font_family(crate::design::mono())")
            .count(),
        2,
        "chart stack coin/core titles and their trailing data note must both stay mono"
    );
    for (path, function, surface) in [
        (
            "chrome/clock.rs",
            "fn render_header_clock(",
            "the header timestamp",
        ),
        (
            "chrome/terminal_chrome.rs",
            "fn ticker_readout(",
            "the ticker readout",
        ),
        (
            "chrome/terminal_chrome.rs",
            "fn balance_label(",
            "the balance figure",
        ),
    ] {
        let source = read_src(path);
        assert!(
            code_only(braced_body(&source, function)).contains(".font_family(design::mono())"),
            "{path}:{function} must keep mono for {surface}, which users compare as a figure"
        );
    }
    // `analytics::report_strategy_combo...` already pins report/controls.rs:strategy_combo's
    // `design::mono()` call. Keep that existing assertion as the single oracle for this site.
}

/// Settings changes its root to the UI face, while its input and numeric value containers remain data.
///
/// Breakage: omitting one re-pin makes a typed value, slider endpoint, or counter render in Inter
/// after the Settings root changes, despite users reading it as a comparable value.
#[test]
fn settings_values_and_connections_repin_the_mono_family() {
    let general = read_src("settings/general.rs");
    for (function, mono) in [
        ("pub(super) fn font_delta_control(", ".mono(true)"),
        (
            "pub(super) fn stepper_controls(",
            ".font_family(design::mono())",
        ),
    ] {
        assert!(
            code_only(braced_body(&general, function)).contains(mono),
            "settings/general.rs:{function} must explicitly keep its editable or stepped value mono"
        );
    }
    assert!(
        code_only(braced_body(&general, "fn font_delta_marks("))
            .contains(".font_family(design::mono())"),
        "settings/general.rs:font_delta_marks must keep its compared tick numbers mono"
    );
    let badges = read_src("settings/badges.rs");
    let badge_row = code_only(braced_body(&badges, "fn badge_row("));
    assert_eq!(
        badge_row.matches(".mono(true)").count(),
        5,
        "the ordinal, name, code, optional short-code, and status badge values must all stay mono after Settings becomes proportional"
    );
    let common = read_src("settings/common.rs");
    assert!(
        code_only(braced_body(&common, "pub(super) fn slider_row("))
            .contains(".font_family(design::mono())"),
        "slider endpoints and current numeric value must share a mono container after Settings becomes proportional"
    );
    let settings = read_src("settings/render.rs");
    let render = render_root(&settings, "impl Render for SettingsView");
    let connections = chain_between(
        &render,
        "if self.active == Tab::Connections {",
        "} else {",
        "Settings Connections body",
    );
    assert!(
        connections.contains(".font_family(design::mono())"),
        "Settings Connections must pin mono at its branch because its table inherits the Settings root otherwise"
    );

    let security = read_src("settings/security.rs");
    assert!(
        code_only(braced_body(&security, "fn password_row(")).contains(".mono(true)"),
        "settings/security.rs:password_row must keep every revealable secret mono"
    );
    assert!(
        code_only(braced_body(&security, "fn machines_row("))
            .contains(".font_family(design::mono())"),
        "settings/security.rs:machines_row must keep its machine-slot figures mono"
    );
    assert!(
        code_only(braced_body(&security, "fn strength_meter("))
            .contains(".font_family(design::mono())"),
        "settings/security.rs:strength_meter must keep its security.bits entropy figure mono"
    );

    let import_preview = read_src("settings/import_preview.rs");
    assert!(
        code_only(braced_body(&import_preview, "fn value_el("))
            .contains(".font_family(design::mono())"),
        "settings/import_preview.rs:value_el must keep old and new setting values mono"
    );

    let hotkeys = read_src("settings/hotkeys/tab.rs");
    let pull_row = code_only(braced_body(&hotkeys, "fn core_pull_row("));
    let pull_slot = chain_between(
        &pull_row,
        "let id = format!(\"core-pull-{}\", registry::key_id(row.slot));",
        "MoonHotkeyInput::new",
        "core hotkey slot identity",
    );
    assert!(
        pull_slot.contains(".font_family(design::mono())"),
        "settings/hotkeys/tab.rs:core_pull_row must keep the compared slot identity mono"
    );

    let tree = read_src("strategies/tree/mod.rs");
    let action_bar = code_only(braced_body(&tree, "fn action_bar("));
    let staged_slot = chain_between(
        &action_bar,
        "let staged_slot = div()",
        ".child(staged_label.unwrap_or_default())",
        "Strategies staged-count slot",
    );
    assert!(
        staged_slot.contains(".font_family(design::mono())"),
        "strategies/tree/mod.rs:action_bar must keep its staged count mono"
    );

    let strategy_settings = read_src("strategies/settings.rs");
    let settings_width = code_only(braced_body(
        &strategy_settings,
        "fn settings_content_width(",
    ));
    let group_width = chain_between(
        &settings_width,
        "let group_width =",
        "let checkbox_label_width =",
        "Strategies group-caption measurement",
    );
    assert!(
        group_width.contains("true"),
        "strategies/settings.rs:group_width must measure MoonGroupBox's hardcoded mono caption"
    );
}

/// Widths and rendered control captions must use one font family, while row-fit keeps its mono cache identity.
///
/// Breakage: changing only one side shifts the quiet toggle and ticker popup, or preserves stale
/// header shedding thresholds after a mono/UI-family font change.
#[test]
fn width_measurements_agree_with_their_rendered_family_and_cache_key() {
    let quiet = read_src("chrome/quiet.rs");
    assert!(
        code_only(braced_body(&quiet, "pub(crate) fn header_quiet_width("))
            .contains("design::ui_caption_text_width("),
        "header_quiet_width must measure the UI caption family because shell::ticker reuses this width as its popup offset"
    );
    assert!(
        code_only(braced_body(&quiet, "pub(crate) fn header_quiet_cluster("))
            .contains(".font_family(design::ui_font())"),
        "the quiet toggle caption must render in the UI family that header_quiet_width measures"
    );
    let wrap_fit = read_src("controls/wrap_fit.rs");
    let signature = code_only(braced_body(&wrap_fit, "pub(crate) fn signature("));
    assert!(
        signature.contains("design::text_metrics_key(cx, design::ACTION_LABEL_BASE, 400.0, true)"),
        "row-fit signature must hash the mono family used by its Report filter caller"
    );
}
