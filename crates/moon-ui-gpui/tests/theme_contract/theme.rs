//! Theme and chart-surface bans: the runtime MoonUI theme is the only palette, one mapping turns
//! a profit-and-loss sign into a tone, and the GPU chart keeps its own pass under the scene.

use super::support::*;

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

/// Every money cell that states an unrealized PnL must resolve its tone through
/// `design::delta_tone`, and must not pick a colour by comparing the raw amount against zero.
///
/// This is the defect that started the shared helpers, and it was found by GREP rather than by a
/// test: each surface hand-rolled `if v >= 0.0 { Positive } else { Danger }` beside its own
/// `format!("+{v:.2}")`. Two things go wrong the moment they drift apart. A `-0.004` prints as
/// `0.00` while `v < 0.0` still paints it red, so a break-even figure reads as a loss; and a
/// literal `-0.0` passes `v >= 0.0`, so the other spelling renders `+-0.00`. Routing the tone from
/// the `DeltaSign` the formatter returns makes both unrepresentable, because the sign is
/// classified from the value the TEXT was rendered from.
///
/// `moon-ui-gpui` is a binary crate with no `[lib]`, so this is checked as text. Each body is
/// stripped through [`code_only`] first: `braced_body` returns comments too, and the comment above
/// each of these cells names `delta_tone`, so a substring assertion would pass with the call
/// deleted. `MoonTone::Muted` stays legal — it is the dash these cells show when there is no
/// figure at all, not a sign-derived colour.
#[test]
fn pnl_money_cells_take_their_tone_from_the_shared_delta_mapping() {
    // Every money cell that states an unrealized PnL, by file and by function signature.
    const CELLS: [(&str, &str); 4] = [
        ("panels/orders/table.rs", "fn pnl_cell("),
        ("panels/orders/table.rs", "fn pnl_tp_cell("),
        ("panels/orders/table.rs", "fn pnl_pct_cell("),
        ("panels/assets/table.rs", "fn pnl_cell("),
    ];

    for (rel, signature) in CELLS {
        let source = read_src(rel);
        // `braced_body` takes the FIRST match, so a signature that appears twice would silently
        // check the wrong body — and a renamed cell would make every assertion below vacuous.
        assert_eq!(
            source.matches(signature).count(),
            1,
            "{rel}: `{signature}` must name exactly one money cell"
        );
        let body = code_only(braced_body(&source, signature));

        assert!(
            body.contains("design::delta_tone("),
            "{rel}:{signature} must resolve its tone through design::delta_tone"
        );
        for hand_rolled in ["MoonTone::Positive", "MoonTone::Danger"] {
            assert!(
                !body.contains(hand_rolled),
                "{rel}:{signature} must not name {hand_rolled} itself; delta_tone owns that mapping"
            );
        }
        // A raw comparison against zero is how the sign got out of step with the text: these all
        // read the UNROUNDED amount, which the rendered digits no longer represent.
        for raw_compare in [">= 0.0", "> 0.0", "< 0.0", "<= 0.0"] {
            assert!(
                !body.contains(raw_compare),
                "{rel}:{signature} must not compare the raw amount `{raw_compare}` to pick a colour"
            );
        }
        // The text must come from the same rounding the sign does, not from a local `format!`.
        assert!(
            !body.contains("format!(\"+"),
            "{rel}:{signature} must not hand-format its sign; fmt::signed_* returns both halves"
        );
    }
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
    let detached = fs::read_to_string(root.join("window").join("detached.rs")).unwrap();

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
    // Ctrl+right-click is a trading gesture slot, and on macOS the OS-style Ctrl+LEFT click used to
    // arrive here as a plain right click: without this guard the stack collapsed under a user who
    // was moving an order. Moonbot toggles on the unmodified click too.
    assert!(
        main_stack.contains("!event.modifiers.modified()"),
        "Main stack RMB fullscreen/stack toggle must ignore a right-click carrying a modifier"
    );
}

/// Settings-popup contents hosted by `MoonPopover` must not paint a second surface inside it.
///
/// `MoonPopover` already draws the background, border, radius and outer padding, and its
/// `content_width_*` builders treat the number they are given as the width of the CONTENT box.
/// A content root that paints its own chrome therefore both nests a frame inside a frame and
/// silently narrows the real content below the width its own constant declares — the drift this
/// pins shipped in two of these popups at once, so prose in the module header did not hold it.
///
/// Checked on the root builder chain only (from the root's `.id(..)` to its first `.child(..)`),
/// because nested cards and banners inside these popups legitimately paint their own fills.
///
/// Known blind spots, all deliberate rather than overlooked: chrome chained onto the root AFTER its
/// first child is outside the slice, and the registry below covers only popovers that declare a
/// content width — a menu-style popover (`fit_content`, `width`) carries no width constant for a
/// second frame to invalidate, which is what makes this rule bite.
#[test]
fn popover_contents_do_not_paint_a_second_surface() {
    // (file declaring the popover, file building its content, the anchor that starts the content
    // root's builder chain). The first two differ where a popup's content lives apart from its
    // trigger; the anchor is spelled out because the chart popups build their id with `format!`.
    const ROOTS: &[(&str, &str, &str)] = &[
        (
            "chrome/terminal_chrome.rs",
            "shell/core_settings_popup.rs",
            r#".id("core-settings-popup")"#,
        ),
        (
            "panels/detects/popup.rs",
            "panels/detects/popup.rs",
            r#".id("detects-view-popup")"#,
        ),
        // The header's quiet-mode ("sleep") gear. Declared beside the toggle in `chrome/quiet.rs`,
        // built on Shell because its time fields are retained `MoonInputState` entities.
        (
            "chrome/quiet.rs",
            "shell/quiet_popup.rs",
            r#".id("quiet-settings-popup")"#,
        ),
        (
            "panels/news/mod.rs",
            "panels/news/mod.rs",
            r#".id("news-tags-content")"#,
        ),
        (
            "panels/core_status/config_popup.rs",
            "panels/core_status/config_popup.rs",
            r#".id("core-status-warn-content")"#,
        ),
        (
            "analytics/tuner/shell.rs",
            "analytics/tuner/shell.rs",
            r#".id("tun-cfg-popup")"#,
        ),
        (
            "controls/metric.rs",
            "controls/metric.rs",
            r#".id("metric-popup-content")"#,
        ),
        (
            "chart_tabs/common.rs",
            "chart_tabs/layout_popup.rs",
            r#".id(SharedString::from(format!("{id}-popup")))"#,
        ),
        (
            "chart_tabs/candle_popup.rs",
            "chart_tabs/candle_popup.rs",
            r#".id(SharedString::from(format!("{id}-popup")))"#,
        ),
        (
            "chart_tabs/graphics_popup.rs",
            "chart_tabs/graphics_popup.rs",
            r#".id(SharedString::from(format!("{id}-popup")))"#,
        ),
        (
            "chart_tabs/labels_popup/mod.rs",
            "chart_tabs/labels_popup/mod.rs",
            r#".id(SharedString::from(format!("{id}-popup")))"#,
        ),
        (
            "analytics/profit_monitor/settings.rs",
            "analytics/profit_monitor/settings.rs",
            r#".id("profit-monitor-settings-popup")"#,
        ),
        (
            "strategies/settings.rs",
            "strategies/settings.rs",
            r#".id("strategies-settings-popup")"#,
        ),
        (
            "panels/assets/settings.rs",
            "panels/assets/settings.rs",
            r#".id("assets-wallets-settings-popup")"#,
        ),
        (
            "settings/connections/tab.rs",
            "settings/connections/tab.rs",
            r#".id("icon-picker")"#,
        ),
        // The manual-strategy quick-select gear: its content lives in the cluster's own submodule,
        // so the declaring file and the content file differ.
        (
            "controls/manual_strat.rs",
            "controls/manual_strat/settings.rs",
            r#".id("ms-slots-popup")"#,
        ),
        // The Alerts row gear. Its content is `figstyle::rows` BARE — the same rows the chart's
        // own settings panel wraps in `figstyle::shell`, which is what paints a surface there and
        // must not be handed to a popover that already paints one. The anchor is the content
        // root's own binding: these rows carry no `.id(..)`, having no state to retain.
        (
            "panels/alerts/table.rs",
            "figstyle/mod.rs",
            "let mut rows = v_flex()",
        ),
    ];
    // Every one of these is chrome `MoonPopover` already paints around the content. The `_`-suffixed
    // entries are the tailwind-style shorthands, which re-add the same padding by another spelling.
    //
    // `font_value` is banned for a different reason, and it is the one that actually shipped: the
    // popover resolves `content_width_font` through `font_width` (a pure multiply), while
    // `design::font_value` is the ADDITIVE `font()` used for text SIZES. A content root that sets
    // its own width with it lands ~47px off the frame at the shipped +3 Font delta. A content root
    // needs no width of its own at all — `w_full` — and the two callers that do state one use
    // `font_w_px`, which is the same formula the popover uses.
    const BANNED: &[&str] = &[
        "font_value(",
        ".bg(",
        ".border(",
        ".border_1(",
        ".rounded(",
        ".rounded_",
        ".p(",
        ".p_",
        ".px(",
        ".px_",
        ".py(",
        ".py_",
    ];

    for (_, rel, anchor) in ROOTS {
        let source = read_src(rel);
        let chain = chain_between(&source, anchor, ".child(", rel);
        for banned in BANNED {
            assert!(
                !chain.contains(banned),
                "{rel}: popup content root `{anchor}` calls `{banned}`, but MoonPopover already \
                 paints that chrome — drawing it here doubles the frame and narrows the content \
                 box below the width the popup's own constant declares"
            );
        }
    }

    // The list above is a registry, so it has to grow with the code rather than guard whatever
    // someone remembered to add. Every source that declares a content-width popover must appear.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);
    for path in sources {
        let text = fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!("failed to read {}: {err}", path.display());
        });
        // Two spellings count as declaring one: a popover built inline, and a gear built through
        // the header's shared `header_gear_popover`. Without the second, moving a popup onto the
        // helper would quietly take it out of this registry — which is exactly what the helper
        // makes easy to do.
        let inline = text.contains("MoonPopover::new") && text.contains(".content_width");
        if !inline && !text.contains("header_gear_popover(") {
            continue;
        }
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        assert!(
            ROOTS.iter().any(|(declared_in, ..)| *declared_in == rel),
            "{rel} declares a content-width MoonPopover but is missing from this test's ROOTS — \
             add its content root so the no-second-chrome rule covers it too"
        );
    }
}

/// `settings/connections/table.rs:SettingsView::cell` must apply a column's resolved cap through
/// `max_w`. Deleting that match lets Group consume wide-window space past its 140px cap and
/// truncates the uncapped Name field.
#[test]
fn connections_cells_apply_their_resolved_growth_cap() {
    let table = read_src("settings/connections/table.rs");
    let cell = code_only(braced_body(
        &table,
        "fn cell(col: ConnColId, micro: MicroTriggerMetrics) -> Div",
    ));

    assert!(
        cell.contains("let d = match col.max_width(micro) {")
            && cell.contains("Some(max) => d.max_w(px(max)),"),
        "SettingsView::cell must resolve ConnColId::max_width and apply it with Div::max_w"
    );
}

// --- Goal A: header/toolbar chrome polish -----------------------------------------------------
//
// These four are text-based checks against symbols that do NOT exist in the tree yet
// (`design::readout_color`, `design::chrome_toggle_tone`) or against call forms the current
// production code does not use yet (`design::delta_tone` in `ticker_readout`,
// `design::fit_h_value` in `core_selector`). Every check below is a plain substring search over
// source text, so all four COMPILE against today's tree exactly as the existing tests in this
// file do; they are simply expected to FAIL until the implementation lands, which is the
// documented pre-implementation state for an AUTHOR-mode packet, not a defect in the test.

/// The header ticker's 1h/24h percentage deltas must resolve their colour through
/// `design::delta_tone(sign)`, the single documented sign-to-tone mapping, and never through a
/// hand-picked `MoonTone` or a hand-rolled `sign.pick(..)`.
///
/// The future edit this pins against: replacing `design::delta_tone(sign)` with
/// `sign.pick(MoonTone::Positive, MoonTone::Negative, MoonTone::Muted)` — a plausible edit,
/// because "Negative" reads like the right name for a loss. `MoonTone::Negative` is ORANGE in the
/// dark theme, not red, so a losing delta would render orange while every other money cell in the
/// app renders red. `delta_span` is a closure INSIDE `ticker_readout`, so a `braced_body` grab of
/// the enclosing fn covers it, exactly as it does for [`pnl_money_cells_take_their_tone_from_the_shared_delta_mapping`]
/// above.
#[test]
fn header_ticker_deltas_take_their_tone_from_the_shared_delta_mapping() {
    let source = read_src("chrome/terminal_chrome.rs");
    let signature = "fn ticker_readout(";
    assert_eq!(
        source.matches(signature).count(),
        1,
        "`{signature}` must name exactly one header ticker readout"
    );
    let body = code_only(braced_body(&source, signature));

    assert!(
        body.contains("design::delta_tone("),
        "chrome/terminal_chrome.rs:{signature} must resolve its delta colour through design::delta_tone"
    );
    for hand_rolled in [
        "MoonTone::Positive",
        "MoonTone::Danger",
        "MoonTone::Negative",
    ] {
        assert!(
            !body.contains(hand_rolled),
            "chrome/terminal_chrome.rs:{signature} must not name {hand_rolled} itself; delta_tone owns that mapping"
        );
    }
    assert!(
        !body.contains("sign.pick("),
        "chrome/terminal_chrome.rs:{signature} must not hand-roll a sign.pick(..) tone selection"
    );
}

/// The header's core pill must size its in-flow trigger height with the same fit triple as the
/// Small buttons beside it — `design::fit_h_value(cx, SEL_H, 14.0, 6.0)` — never a flat
/// `design::ui_value(cx, SEL_H)`.
///
/// The future edit this pins against: reverting to `design::ui_value(cx, SEL_H)`, which looks
/// equivalent and is the more obvious of the two calls, and is what the tree ships today.
/// MoonUI's `ToolbarCompact`/`Action` buttons resolve through `fit_height(26,14,6)` and land at 29
/// at the default +3 font delta and 32 at +6, while a flat `ui_value(26)` stays 26 — the pill
/// would then sit 3-6px shorter than every neighbour in the same band.
#[test]
fn header_core_pill_shares_the_toolbar_buttons_fit_rule() {
    let source = read_src("chrome/terminal_chrome.rs");
    let signature = "fn core_selector(";
    assert_eq!(
        source.matches(signature).count(),
        1,
        "`{signature}` must name exactly one core-pill builder"
    );
    let body = code_only(braced_body(&source, signature));

    assert!(
        body.contains("design::fit_h_value(cx, SEL_H, 14.0, 6.0)"),
        "chrome/terminal_chrome.rs:{signature} must size the pill trigger with fit_h_value(cx, SEL_H, 14.0, 6.0)"
    );
    assert!(
        !body.contains("design::ui_value(cx, SEL_H)"),
        "chrome/terminal_chrome.rs:{signature} must not fall back to a flat ui_value(cx, SEL_H)"
    );
}

/// Every toolbar readout that can be absent — Lev, the exchange max-order figure, and SL — must
/// resolve its colour through `design::readout_color`, never a bare `p.text` that renders a live
/// figure and an unreported "–" the same weight (the whole of acceptance item A2).
///
/// The future edit this pins against: a call site reverting to a bare `p.text` — which is exactly
/// what the Lev and max-order sites do today (`p.text` unconditionally, at the current
/// `metric_button(TradeMetric::Lev, ..)` and `strip_text(max_order_value, p.text)` call forms).
/// Counted rather than location-pinned: `pub fn toolbar` is one long builder, and grabbing its
/// whole body survives the exact call sites moving a few lines as the implementation lands.
#[test]
fn toolbar_unset_readouts_resolve_their_colour_through_readout_color() {
    let source = read_src("controls/toolbar.rs");
    let signature = "pub fn toolbar(";
    assert_eq!(
        source.matches(signature).count(),
        1,
        "`{signature}` must name exactly one toolbar builder"
    );
    let body = code_only(braced_body(&source, signature));

    assert_eq!(
        body.matches("design::readout_color(").count(),
        3,
        "controls/toolbar.rs:{signature} must route exactly its three optional readouts (Lev, \
         max order, SL) through design::readout_color"
    );
}

/// The three chrome toggles — the header's Sleep toggle, the toolbar's own-trade toggle, and the
/// SL toggle beside it — must resolve their tone through `design::chrome_toggle_tone` rather than
/// naming a `MoonTone` inline, so "amber" keeps meaning "this one is a caution state" everywhere
/// it appears (the whole of acceptance item A3).
///
/// The future edit this pins against: a fourth chrome toggle hand-passing
/// `.tone(MoonTone::Warning)`, or one of these three dropping the call — which is what the Sleep
/// toggle does today (`.tone(if sleeping { MoonTone::Warning } else { MoonTone::Info })` inline)
/// and what the SL toggle does today (no `.tone(..)` call at all).
/// The core pill's `MoonSelectorPill` builder must consume `trigger_h_units` — the DESIGN-unit
/// value undone from the final-pixel `trigger_h` — for both `.height(..)` and `.radius(..)`, never
/// the final-pixel `trigger_h` itself.
///
/// The future edit this pins against: passing `trigger_h` straight into `.radius(..)` (e.g.
/// `.radius(trigger_h / 2.0)`). `MoonSelectorPill::radius` is itself a design-unit input scaled
/// again at `selector.rs`'s own `tokens.ui(self.radius)`, so a final-pixel value there gets scaled
/// twice — invisible at the default `ui_scale = 1.0`, where the double multiply is the identity,
/// and wrong the moment anyone hand-edits the UI slider.
#[test]
fn core_pill_height_and_radius_use_design_units_not_final_pixels() {
    let source = read_src("chrome/terminal_chrome.rs");
    let signature = "fn core_selector(";
    assert_eq!(
        source.matches(signature).count(),
        1,
        "`{signature}` must name exactly one core-pill builder"
    );
    let body = code_only(braced_body(&source, signature));

    assert!(
        body.contains(".height(trigger_h_units)"),
        "chrome/terminal_chrome.rs:{signature} must size the pill with the design-unit trigger_h_units"
    );
    assert!(
        body.contains(".radius(trigger_h_units / 2.0)"),
        "chrome/terminal_chrome.rs:{signature} must radius the pill from trigger_h_units, not final pixels"
    );
    assert!(
        !body.contains(".radius(trigger_h / 2.0)"),
        "chrome/terminal_chrome.rs:{signature} must not radius the pill from the final-pixel \
         trigger_h, which MoonSelectorPill::radius would scale a second time"
    );
}

#[test]
fn chrome_toggles_resolve_their_tone_through_one_helper() {
    const SITES: [(&str, &str); 3] = [
        ("chrome/quiet.rs", "pub(crate) fn header_quiet_cluster("),
        ("controls/toolbar.rs", "fn own_trade_toggle("),
        ("controls/metric.rs", "pub(super) fn sl_toggle("),
    ];

    for (rel, signature) in SITES {
        let source = read_src(rel);
        assert_eq!(
            source.matches(signature).count(),
            1,
            "{rel}: `{signature}` must name exactly one chrome toggle builder"
        );
        let body = code_only(braced_body(&source, signature));
        assert!(
            body.contains("design::chrome_toggle_tone("),
            "{rel}:{signature} must resolve its tone through design::chrome_toggle_tone"
        );
    }
}
