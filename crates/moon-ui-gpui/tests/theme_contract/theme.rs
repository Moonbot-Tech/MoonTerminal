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
            "analytics/profit_monitor/settings.rs",
            "analytics/profit_monitor/settings.rs",
            r#".id("profit-monitor-settings-popup")"#,
        ),
        (
            "strategies/settings.rs",
            "strategies/settings.rs",
            r#".id("strategies-settings-popup")"#,
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
    // its own width with it lands ~47px off the frame at the shipped +2 Font delta. A content root
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
