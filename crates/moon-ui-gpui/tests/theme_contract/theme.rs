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
    let main_stack =
        fs::read_to_string(root.join("chart_tabs").join("main_stack").join("tabs.rs")).unwrap();

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
        // The Entry/Exit axis' two popovers — the search settings and the model settings — share
        // one content root.
        (
            "analytics/tuner/ticks/cfg.rs",
            "analytics/tuner/ticks/cfg.rs",
            "fn popup_frame(",
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
            "chart_tabs/history_popup.rs",
            "chart_tabs/history_popup.rs",
            r#".id(SharedString::from(format!("{id}-popup")))"#,
        ),
        (
            "chart_tabs/volumes_popup.rs",
            "chart_tabs/volumes_popup.rs",
            r#".id(SharedString::from(format!("{id}-popup")))"#,
        ),
        (
            "chart_tabs/labels_popup/mod.rs",
            "chart_tabs/labels_popup/mod.rs",
            r#".id(SharedString::from(format!("{id}-popup")))"#,
        ),
        // The trade window's one ⚙: the chart popups' rows for a frozen replay, in one place.
        (
            "trade_window/settings.rs",
            "trade_window/settings.rs",
            r#".id("trade-window-settings-popup")"#,
        ),
        (
            "analytics/profit_monitor/settings.rs",
            "analytics/profit_monitor/settings.rs",
            r#".id("profit-monitor-settings-popup")"#,
        ),
        // The empty Main screen's ⚙. Its ids are built per group, like every other identity in
        // that stack, so the anchor is the builder call rather than a literal.
        (
            "chart_tabs/main_stack/empty.rs",
            "chart_tabs/main_stack/empty.rs",
            r#".id(id("settings-popup"))"#,
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
        // The Cores page's group icon picker: declared on the page, its grid built in the shared
        // tree helpers.
        (
            "settings/connections/page.rs",
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
        // The Auto rail's gear: run-control preferences, same shape as the Profit Monitor popup.
        (
            "shell/workspace/rail_settings.rs",
            "shell/workspace/rail_settings.rs",
            r#".id("workspace-rail-settings-popup")"#,
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
/// Small buttons beside it — `design::action_control_h_value(cx)` — never a flat copied height.
///
/// The future edit this pins against: returning to a copied `SEL_H` constant, which stops following
/// the control tier. The pill would then sit shorter than neighbouring action controls.
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
        body.contains("let trigger_h = design::action_control_h_value(cx);"),
        "chrome/terminal_chrome.rs:{signature} must size the pill trigger from the control-tier action control"
    );
    assert!(
        !body.contains("SEL_H"),
        "chrome/terminal_chrome.rs:{signature} must not fall back to a copied fixed selector height"
    );
}

/// Every toolbar readout that can be absent — Lev, the exchange max-order figure, and SL — must
/// resolve its colour through `design::readout_color`, never a bare `p.text` that renders a live
/// figure and an unreported "–" the same weight (the whole of acceptance item A2).
///
/// The future edit this pins against: a call site reverting to a bare `p.text` — which is exactly
/// what the Lev and max-order sites do today (`p.text` unconditionally, at the current
/// `metric_button(TradeMetric::Lev, ..)` and `strip_text(max_order_value, p.text)` call forms).
/// Counted rather than location-pinned: `render_toolbar` is one long builder, and grabbing its
/// whole body survives the exact call sites moving a few lines.
#[test]
fn toolbar_unset_readouts_resolve_their_colour_through_readout_color() {
    let source = read_unsplit("controls/toolbar.rs");
    let signature = "pub(crate) fn render_toolbar(";
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

/// The core card must take its height as the final-pixel `trigger_h` straight into `px(..)`, never
/// through a second UI-scale pass, and its corners from the shared button radius.
///
/// The future edit this pins against: feeding `trigger_h` to something that scales it again (a
/// `MoonSelectorPill::height`/`radius`, or `design::ui_px(cx, trigger_h)`), which is invisible at
/// the default `ui_scale = 1.0`, where the double multiply is the identity, and wrong the moment
/// anyone moves the UI slider.
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
        body.contains(".h(px(trigger_h))"),
        "chrome/terminal_chrome.rs:{signature} must size the card with the final-pixel trigger_h"
    );
    assert!(
        body.contains(".rounded(design::r_button(cx))"),
        "chrome/terminal_chrome.rs:{signature} must round the card with the shared button radius"
    );
    assert!(
        !body.contains("ui_px(cx, trigger_h") && !body.contains("MoonSelectorPill"),
        "chrome/terminal_chrome.rs:{signature} must not scale the final-pixel trigger_h a second time"
    );
}

/// The chrome toggles — the Stop pod's mini switch (the header's Sleep control is a labelled
/// button, accent-filled while asleep) — must resolve their tone through
/// `design::chrome_toggle_tone` rather than naming a `MoonTone` inline, so "amber" keeps meaning
/// "this one is a caution state" everywhere it appears (the whole of acceptance item A3). The
/// own-set control is an icon button rather than a toggle, filled while on; it must not grow a
/// hand-toned switch back.
///
/// The future edit this pins against: a chrome toggle hand-passing `.tone(MoonTone::Warning)`, or
/// the SL switch dropping the call.
#[test]
fn chrome_toggles_resolve_their_tone_through_one_helper() {
    const SITES: [(&str, &str); 1] = [("controls/metric.rs", "pub(super) fn sl_toggle(")];

    for (rel, signature) in SITES {
        let source = read_unsplit(rel);
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
    let toolbar = read_unsplit("controls/toolbar.rs");
    let own = code_only(braced_body(&toolbar, "fn own_trade_toggle("));
    assert!(
        own.contains(r#"MoonButton::new("toolbar-own-trade")"#)
            && own.contains("MoonButtonVariant::Blue")
            && own.contains(".selected(on)")
            && !own.contains("MoonToggle"),
        "the own-set control must stay an icon button whose on state is the Blue fill marked .selected(on)"
    );
    let own_row = code_only(braced_body(&toolbar, "fn own_trade_menu_check("));
    assert!(
        own_row.contains(".checked(on)"),
        "the own-set row in \"More\" must show its state as .checked(on)"
    );
    // The Sleep control is the third chrome two-state control: a labelled button whose asleep
    // state is the accent fill plus `selected`, never a hand-picked tone.
    let quiet = code_only(braced_body(
        &read_src("chrome/quiet.rs"),
        "pub(crate) fn header_quiet_cluster(",
    ));
    assert!(
        quiet.contains("MoonButtonVariant::Blue")
            && quiet.contains(".selected(sleeping)")
            && !quiet.contains(".tone("),
        "chrome/quiet.rs:header_quiet_cluster must fill the asleep button with MoonButtonVariant::Blue and mark it .selected, with no hand-picked tone"
    );
}

/// `settings/mod.rs:draft_sig` must stream serde into its hasher rather than
/// materializing plaintext `Secret` values. Breakage: replacing the writer with `to_string`,
/// `to_vec`, or debug formatting keeps every core key in an unzeroized allocation on repaint.
#[test]
fn connections_dirty_signature_never_materializes_secret_config_text() {
    let dirty = read_src("settings/mod.rs");
    let hash_json = code_only(braced_body(&dirty, "fn hash_json<"));

    assert!(
        hash_json.contains("serde_json::to_writer("),
        "settings/mod.rs:hash_json must stream serialized settings into its hash sink"
    );
    for allocating in ["serde_json::to_string(", "serde_json::to_vec("] {
        assert!(
            !hash_json.contains(allocating),
            "settings/mod.rs:hash_json must not materialize secret-bearing config through `{allocating}`"
        );
    }
}

/// `settings/mod.rs:draft_sig` must account for every `AppConfig::blank` field or name it in its
/// explicit exclusion list. Breakage: adding a Settings-editable config field without updating the
/// signature makes its edits read clean forever, so close discards them without a compile error.
#[test]
fn settings_dirty_signature_covers_or_explicitly_excludes_every_config_field() {
    let config = read_core_src("config/mod.rs");
    let blank = code_only(braced_body(&config, "pub(in crate::config) fn blank("));
    let settings = read_src("settings/mod.rs");
    let signature = code_only(braced_body(&settings, "fn draft_sig("));
    let excluded = ["next_uid", "settings_unreadable", "chart_core_remap_needed"];
    let fields = blank
        .lines()
        .filter_map(|line| line.trim().split_once(':').map(|(field, _)| field.trim()))
        .filter(|field| {
            !field.is_empty()
                && field
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
        .collect::<Vec<_>>();

    assert!(
        !fields.is_empty(),
        "moon-core AppConfig::blank must spell out config fields for this coverage contract"
    );
    assert!(
        settings.contains("deliberately EXCLUDED"),
        "settings/mod.rs:draft_sig must keep an explicit EXCLUDED list for fields it intentionally omits"
    );
    for field in excluded {
        assert!(
            settings.contains(field),
            "settings/mod.rs:draft_sig must document its exclusion of AppConfig::{field}"
        );
    }
    for field in fields {
        let covered = match field {
            "groups" => signature.contains("canonical_groups("),
            _ => excluded.contains(&field) || signature.contains(field),
        };
        assert!(
            covered,
            "settings/mod.rs:draft_sig must hash or explicitly exclude AppConfig::{field}"
        );
    }
}

/// `connections/mod.rs:build_conn` must put key and bundle placeholders on their input state,
/// while `cores.rs:core_card` keeps its state-bound inputs free of widget placeholders. Breakage:
/// moving either placeholder back to `MoonInput` drops it silently, leaving an empty field blank.
#[test]
fn connections_placeholders_live_on_state_not_state_bound_widgets() {
    let connections = read_src("settings/connections/mod.rs");
    let build_conn = code_only(braced_body(&connections, "fn build_conn("));
    let cores = read_src("settings/connections/cores.rs");
    let card = code_only(braced_body(&cores, "pub(crate) fn core_card("));

    assert!(
        build_conn.contains("conn.key_ph") && build_conn.contains("conn.bundle_ph"),
        "connections/mod.rs:build_conn must seed both state-owned placeholders"
    );
    assert!(
        !card.contains(".placeholder("),
        "connections/cores.rs:core_card must not put placeholders on state-bound MoonInput widgets"
    );
}

/// `connections/cores.rs:core_card` must keep the explanations of its compact controls: the
/// protocol's long text and the feed-flags hint. Breakage: removing either leaves `V0` or `8 из 9`
/// undecodable on the card.
#[test]
fn connections_compact_cells_keep_their_explanatory_tooltips() {
    let cores = read_src("settings/connections/cores.rs");
    let card = code_only(braced_body(&cores, "pub(crate) fn core_card("));

    assert!(
        card.contains("conn.tip.proto"),
        "connections/cores.rs:core_card must keep the protocol's explanation"
    );
    assert!(
        card.contains("conn.card.feeds_hint"),
        "connections/cores.rs:core_card must keep the feed-flags hint"
    );
}

/// Restoring a fixed or capped popup width clips the longest localized option beside its check.
/// Every core-card menu (protocol, total mode, group) goes through `cores.rs:dropdown`, which must
/// stay on MoonUI's content measurement at the active menu density.
#[test]
fn connections_total_and_preset_menus_fit_all_localized_rows() {
    let cores = read_src("settings/connections/cores.rs");
    let body = code_only(braced_body(&cores, "fn dropdown("));
    assert!(
        body.contains(".fit_menu_width(0.0, f32::INFINITY)"),
        "cores.rs:dropdown must fit its localized labels, check column and padding without a width cap"
    );
    for fixed_width in [".menu_width(", ".menu_width_scaled("] {
        assert!(
            !body.contains(fixed_width),
            "cores.rs:dropdown must not override its content-fitted popup with {fixed_width}"
        );
    }
}

/// `settings/frame.rs:save_bar` must disable Save exactly when nothing is unsaved and say so; the
/// dirty arm is chosen by `SettingsView::config_unsaved` through `bar_dirty` — what Save writes,
/// including edits no page names (they read "Есть несохранённые изменения"), never a station draft
/// its own pages apply.
/// The approved mockup wins over the earlier no-fence rule (coordinator ruling): clean ->
/// disabled Save + "Все изменения сохранены"; dirty -> enabled Save beside the page list. J3:
/// accepted-but-unwritten security work counts as dirty, so it can never strand a password behind
/// a disabled button (see the dirty-predicate contract below). Breakage: a Save disabled while
/// something is dirty strands the edit; an enabled clean Save hides that nothing is pending.
#[test]
fn settings_save_button_is_disabled_exactly_when_clean() {
    let frame = read_src("settings/frame.rs");
    let bar = code_only(braced_body(&frame, "pub(super) fn save_bar("));
    let dirty_arm = chain_between(
        &bar,
        "dirty: true,",
        "SaveBar::Draft { .. } =>",
        "settings dirty draft bar",
    );
    let clean_arm = chain_between(
        &bar,
        "SaveBar::Draft { .. } =>",
        "SaveBar::Remote(",
        "settings clean draft bar",
    );

    assert!(
        dirty_arm.contains("MoonButton::new(\"save\")") && !dirty_arm.contains(".disabled("),
        "settings/frame.rs:save_bar must keep Save enabled while any page is dirty"
    );
    assert!(
        dirty_arm.contains("settings.savebar.revert"),
        "settings/frame.rs:save_bar must offer revert beside a dirty Save"
    );
    assert!(
        clean_arm.contains("MoonButton::new(\"save\")")
            && clean_arm.contains(".disabled(true)")
            && clean_arm.contains("settings.savebar.clean"),
        "settings/frame.rs:save_bar must disable a clean Save and say the changes are saved"
    );
    assert!(
        dirty_arm.contains("settings.savebar.dirty_other"),
        "settings/frame.rs:save_bar must name unsaved work that no page carries"
    );
    let parts = code_only(braced_body(&frame, "pub(super) fn page_parts("));
    let unsaved = code_only(braced_body(&frame, "pub(super) fn has_unsaved(&self"));
    assert!(
        parts.contains("dirty: self.config_unsaved(cx)"),
        "settings/frame.rs:page_parts must pick the bar's arm from config_unsaved"
    );
    let config_unsaved = code_only(braced_body(&frame, "pub(super) fn config_unsaved(&self"));
    assert!(
        config_unsaved.contains("bar_dirty("),
        "settings/frame.rs:config_unsaved must decide through bar_dirty, which leaves a station draft out of the bar"
    );
    assert!(
        unsaved.contains("self.draft_dirty")
            && unsaved.contains("self.dirty_page_set(cx)")
            && unsaved.contains("self.station_draft_pending()"),
        "settings/frame.rs:has_unsaved must cover the draft, the pages and the station draft"
    );
}

/// `settings/frame.rs:dirty_page_set` must count the Security page dirty while a password change
/// is typed OR accepted but not yet written (J3), and `page_parts` must feed that set into the
/// draft bar. Breakage: comparing only `AppConfig` renders a password change clean, so Save is
/// disabled while it would write new key slots, and a failed key-slot write looks saved.
#[test]
fn settings_save_dirty_predicate_includes_the_pending_security_draft() {
    let frame = read_src("settings/frame.rs");
    let set = code_only(braced_body(&frame, "pub(super) fn dirty_page_set("));
    let parts = code_only(braced_body(&frame, "pub(super) fn page_parts("));
    let security = read_src("settings/security.rs");
    let unsaved = code_only(braced_body(&security, "pub(super) fn has_unsaved("));

    assert!(
        set.contains("self.security.has_unsaved(cx)") && set.contains("Tab::Security"),
        "settings/frame.rs:dirty_page_set must mark Security dirty from security.has_unsaved"
    );
    assert!(
        unsaved.contains("self.unpersisted") && unsaved.contains("self.has_pending_request(cx)"),
        "settings/security.rs:has_unsaved must cover both a typed request and unwritten accepted work"
    );
    assert!(
        parts.contains("self.dirty_page_set(cx)"),
        "settings/frame.rs:page_parts must build the draft bar from dirty_page_set"
    );
}

/// `settings/mod.rs:page_sig` must give every field `draft_sig` hashes to exactly one page, or
/// list it once in `NO_PAGE_FIELDS`; the Telegram aggregate is split field by field. Breakage: a
/// field left out of both makes its edit dirty the draft with no page dot, and a field listed
/// twice puts the dot on two pages.
#[test]
fn settings_page_signature_assigns_every_draft_field_exactly_once() {
    fn uses(body: &str, path: &str) -> usize {
        body.match_indices(path)
            .filter(|(at, _)| {
                body[at + path.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
            })
            .count()
    }
    fn struct_fields(source: &str, signature: &str) -> Vec<String> {
        code_only(braced_body(source, signature))
            .lines()
            .filter_map(|line| line.trim().strip_prefix("pub ")?.split_once(':'))
            .map(|(field, _)| field.trim().to_string())
            .collect()
    }

    let settings = read_src("settings/mod.rs");
    let draft = code_only(braced_body(&settings, "fn draft_sig("));
    let page = code_only(braced_body(&settings, "fn page_sig("));
    let no_page = {
        let start = settings
            .find("const NO_PAGE_FIELDS")
            .expect("settings/mod.rs must declare NO_PAGE_FIELDS");
        let end = start
            + settings[start..]
                .find("];")
                .expect("NO_PAGE_FIELDS must close");
        &settings[start..end]
    };

    let mut fields: Vec<String> = draft
        .match_indices("cfg.")
        .map(|(at, _)| {
            draft[at + 4..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect::<String>()
        })
        .collect();
    fields.sort();
    fields.dedup();
    assert!(fields.len() > 10, "draft_sig fields not found: {fields:?}");
    assert!(
        draft.contains("canonical_groups(cfg)") && uses(&page, "canonical_groups(cfg)") == 1,
        "page_sig must hash canonical_groups(cfg) exactly once"
    );

    for field in fields.iter().filter(|f| *f != "telegram") {
        let count = uses(&page, &format!("cfg.{field}")) + uses(no_page, &format!("\"{field}\""));
        assert_eq!(
            count, 1,
            "AppConfig::{field} must be owned by exactly one page in page_sig or NO_PAGE_FIELDS"
        );
    }

    let schema = read_core_src("config/schema.rs");
    let menu = read_core_src("config/telegram_menu.rs");
    let telegram = struct_fields(&schema, "pub struct TelegramConfig");
    let bot = struct_fields(&menu, "pub struct BotSettings");
    assert!(telegram.contains(&"bot".to_string()) && !bot.is_empty());
    for field in telegram.iter().filter(|f| *f != "bot") {
        assert_eq!(
            uses(&page, &format!("cfg.telegram.{field}")),
            1,
            "TelegramConfig::{field} must be owned by exactly one page in page_sig"
        );
    }
    for field in &bot {
        assert_eq!(
            uses(&page, &format!("cfg.telegram.bot.{field}")),
            1,
            "BotSettings::{field} must be owned by exactly one page in page_sig"
        );
    }
}

/// `settings/hotkeys/tab.rs:confirm_core_pull` writes the core's layout into the Settings DRAFT
/// only. Breakage: writing `b.config.hotkeys` or calling `.save(` makes "Взять раскладку с ядра"
/// persist past Revert and close-without-saving, the bug the page rebuild removed.
#[test]
fn the_core_pull_writes_the_draft_only() {
    let tab = read_src("settings/hotkeys/tab.rs");
    let body = code_only(braced_body(&tab, "fn confirm_core_pull("));
    assert!(
        body.contains("b.preview.as_mut()"),
        "settings/hotkeys/tab.rs:confirm_core_pull must write the draft"
    );
    assert!(
        !body.contains("b.config.hotkeys") && !body.contains(".save("),
        "settings/hotkeys/tab.rs:confirm_core_pull must not touch the saved config or hotkeys.toml"
    );
}
