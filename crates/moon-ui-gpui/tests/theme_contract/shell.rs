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
    let startup = read_startup();
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
        // The OBSERVER is the contract, not what its closure calls the entity: the same observer
        // now also relays captions edited from a chart's own right-click menu, which needs the
        // handle rather than a discarded argument.
        chart_tabs.contains("cx.observe(&main, |this,")
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
        target_setter.contains("Self::classic_trade_core_for_main_transition(")
            && target_setter
                .matches("Self::classic_trade_core_for_main_transition(")
                .count()
                == 1
            && target_setter.contains("self.set_active_trade_core(group, new_core);")
            && target_setter
                .matches("self.set_active_trade_core(group, new_core);")
                .count()
                == 1,
        "one canonical transition decision must guard the sole durable Classic writer"
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

/// `terminal_chrome.rs::header` and `controls/toolbar.rs::toolbar` must keep their Overview gates
/// around `active_trade_core`: restoring a raw read would present one arbitrary server's balance
/// or leverage as the Auto Overview group's figure.
#[test]
fn overview_chrome_and_toolbar_do_not_read_an_arbitrary_trade_core() {
    let header = code_only(braced_body(
        &read_src("chrome/terminal_chrome.rs"),
        "pub fn header(",
    ));
    assert!(
        header.contains(
            "let scoped_core = if b.is_auto_overview_scope(group) {\n            None\n        } else {\n            b.active_trade_core(group)\n        };"
        ),
        "header balance must use no core in Auto Overview before reading active_trade_core"
    );

    let toolbar = code_only(braced_body(
        &read_src("controls/toolbar.rs"),
        "pub fn toolbar(",
    ));
    assert!(
        toolbar.contains("let overview = b.is_auto_overview_scope(group);")
            && toolbar.contains(
                "let focus_core = if overview {\n            None\n        } else {\n            b.active_trade_core(group)\n        };"
            ),
        "toolbar leverage must use no core in Auto Overview before reading active_trade_core"
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

    // And the measurement must still agree with what the FULL clock draws. Asserting on the call
    // chain, not merely that the functions exist: the width or shared renderer could stop calling
    // `clock_parts`, or the full wrapper could select minute precision while the width keeps seconds.
    // Either drift puts the popup off its trigger while a name-only check stays green.
    let clock = read_src("chrome/clock.rs");
    let width = fn_body(&clock, "fn header_clock_width");
    let full = fn_body(&clock, "fn header_clock(");
    let renderer = fn_body(&clock, "fn render_header_clock(");
    assert!(
        width.contains("clock_parts(") && width.contains("ClockPrecision::Seconds"),
        "header_clock_width must measure the shared full-precision clock parts"
    );
    assert!(
        full.contains("render_header_clock(backend, p, ClockPrecision::Seconds, cx)"),
        "header_clock must keep the same seconds precision measured beside the ticker"
    );
    assert!(
        renderer.contains("clock_parts(selected, now, precision)"),
        "both visible clock modes must derive their strings from the measured clock-parts model"
    );
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

    let shared = code_only(&read_src("controls/core_combo.rs"));
    let shared_body = braced_body(&shared, "pub(crate) fn core_combo<");
    assert!(
        shared_body.contains(
            "MoonMenuItem::action_label(format!(\"{id}-exchange-{section_index}\"), exchange_label)"
        ) && shared_body.contains("on_section(exchange_cores.clone(), app);")
            && !shared_body.contains("MoonMenuItem::label(exchange_label)"),
        "every exchange section, including the unnamed one, must be a clickable batch row now — \
         it has no reported exchange identity, but its members are still a batch worth toggling \
         at once"
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

/// The pinned Orders Auto core must fit its live flattened name without widening Classic.
///
/// Plausible regression: removing the Auto-only wrap makes a fitted live name crowd the trailing
/// controls in a narrow dock; applying it to every branch instead changes the interactive Classic
/// toolbar. The live firetest independently verifies that the fitted cap shows the observed name.
#[test]
fn orders_auto_core_selector_fits_live_name_only_in_auto_core() {
    let controls = read_src("panels/orders/controls.rs");
    let render = read_src("panels/orders/render.rs");
    let combo = braced_body(&controls, "pub(super) fn source_combo(");
    let fitted_auto = combo
        .split_once("combo.label(label).when(auto_core, |combo| {")
        .and_then(|(_, rest)| rest.split_once("})"))
        .map(|(body, _)| body)
        .expect("Orders content fitting must stay inside the AutoCore guard");
    let tooltip_auto = combo
        .split_once("if auto_core {")
        .map(|(_, rest)| rest)
        .expect("Orders tooltip host must stay inside the AutoCore branch");

    assert!(
        combo.contains("let auto_core = scope.is_auto_core();")
            && combo.contains("crate::display_text::flatten_lines(name)")
            && fitted_auto.contains("combo.fit_trigger_width(")
            && fitted_auto.contains("crate::controls::CORE_COMBO_TRIGGER_W,")
            && fitted_auto.contains("AUTO_CORE_TRIGGER_MAX_W,")
            && combo.contains("let tooltip = pinned_label.clone();")
            && tooltip_auto.contains(".when_some(tooltip, |host, label|")
            && tooltip_auto.contains("host.tooltip(crate::panels::common::text_tooltip(label))"),
        "Orders AutoCore must fit and expose the complete live name within its bounded budget"
    );
    let max_width = controls
        .lines()
        .find(|line| {
            line.trim_start()
                .starts_with("const AUTO_CORE_TRIGGER_MAX_W: f32 =")
        })
        .and_then(|line| line.split_once('=').map(|(_, value)| value))
        .and_then(|value| value.trim().trim_end_matches(';').parse::<f32>().ok())
        .expect("Orders Auto core maximum width must remain a numeric constant");
    let max_font_scale = 16.5_f32 / 10.5;
    let conservative_source_line_width = max_width * max_font_scale + 16.0;
    assert!(
        conservative_source_line_width <= 420.0,
        "the fitted source needs {conservative_source_line_width}px at the maximum font scale"
    );
    let render_body = braced_body(&render, "fn render(");
    let (auto_layout, classic_tail) = render_body
        .split_once("let controls = if auto_core {")
        .and_then(|(_, rest)| rest.split_once("} else {"))
        .expect("Orders must isolate the wrapping AutoCore layout from Classic");
    let classic_layout = classic_tail
        .split_once("};")
        .map(|(body, _)| body)
        .expect("Orders Classic layout branch must end before the table");
    assert!(
        render_body.contains(
            "let auto_core = self.effective_scope(self.backend.read(cx)).is_auto_core();"
        ) && auto_layout.contains("controls.flex_wrap()")
            && auto_layout.contains(".ml_auto()")
            && auto_layout.contains(".child(self.columns_menu(cx))")
            && auto_layout.contains(".child(self.sort_menu(cx))")
            && classic_layout.contains(".child(div().flex_1())")
            && !classic_layout.contains(".flex_wrap()")
            && !classic_layout.contains(".ml_auto()")
            && !render_body.contains("overflow_x_scroll"),
        "only AutoCore may wrap its right-aligned Orders action group"
    );
    assert!(
        combo.contains(".disabled(workspace_owned)")
            && combo.contains("if workspace_owned {")
            && combo.contains("&effective_selection")
            && combo.contains("&self.sel_cores")
            && combo.contains("view.update(app, |t, c| t.toggle_core(id, c));")
            && combo.contains("t.toggle_exchange_cores(exchange_cores, c);")
            && !combo.contains("overflow_x_scroll"),
        "content fitting must not alter Orders scope, Classic callbacks, or narrow-row policy"
    );
}

/// The Assets Wallets-section header caret must stay a passive `MoonDisclosure::glyph`.
///
/// Plausible edit: swap `MoonDisclosure::glyph(!collapsed)` for `MoonDisclosure::button(id, ..)`
/// so the caret "looks" clickable on its own. In this fork `should_insert_hitbox` inserts a
/// hitbox for a cursor, a hover style OR a listener — `button` installs all three — so the caret
/// would then own a hitbox that swallows the click meant for the enclosing `assets-wallets-toggle`
/// row, which is what actually flips `wallets_collapsed`. Clicking the label beside the caret
/// still works, so this is invisible without clicking the caret itself.
#[test]
fn the_assets_wallets_header_caret_stays_passive() {
    let table = read_src("panels/assets/table.rs");
    let body = code_only(braced_body(&table, "pub(super) fn bottom("));
    let chain = chain_between(
        &body,
        "\"assets-wallets-toggle\"",
        "\"assets.wallets_hint\"",
        "the Assets wallets header toggle row",
    );
    assert!(
        chain.contains("MoonDisclosure::glyph(!collapsed)"),
        "the wallets header caret must stay a passive MoonDisclosure::glyph, not ::button"
    );
    assert!(
        !chain.contains("MoonDisclosure::button("),
        "an interactive MoonDisclosure::button here would take a hitbox and swallow the row's click"
    );
}

/// The Assets wallet roster groups core rows by venue identity and resolves logos only after an
/// off-thread prewarm, while every core keeps the trust-aware balance figure and click behavior.
#[test]
fn assets_wallet_roster_reuses_canonical_exchange_sections_and_logos() {
    let assets = read_src("panels/assets/mod.rs");
    let table = read_src("panels/assets/table.rs");
    let constructor = braced_body(&assets, "pub(super) fn new(");
    let bottom = braced_body(&table, "pub(super) fn bottom(");

    for needle in [
        "cx.background_spawn(async { crate::media::exchange_logos::prewarm() })",
        "this.exchange_logos_ready = true",
        "cx.notify()",
    ] {
        assert!(
            constructor.contains(needle),
            "Assets prewarm must retain {needle:?}"
        );
    }
    for needle in [
        "crate::core_order::exchange_sections(",
        "crate::controls::venue_section_label(venue)",
        ".then_some(venue)",
        ".and_then(|venue| venue.brand())",
        "crate::media::exchange_logos::exchange_logo",
        "img(logo)",
        "super::balances::figure(Some(agg), p, cx)",
        ".on_click(cx.listener(move |this",
        "this.overview_wallet_pick = Some(cid)",
        "this.selected_core = Some(cid)",
        ".min_w(px(240.0))",
        ".flex_shrink_1()",
    ] {
        assert!(
            bottom.contains(needle),
            "grouped Assets roster must retain {needle:?}"
        );
    }
    assert!(
        bottom.contains("asset-exchange-unknown") && !bottom.contains("status_dot"),
        "unknown exchange headings stay explicit without a fake logo or status dot"
    );
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

/// `panels/log/controls.rs:source_combo` must keep raw identity inside the known exchange action;
/// replacing it with a label makes the heading inert, while storing the formatted label makes its
/// live membership empty even though the selector still reads correctly.
#[test]
fn log_exchange_headers_select_a_live_exchange_aggregate() {
    let controls = read_src("panels/log/controls.rs");
    let combo = braced_body(&controls, "pub(super) fn source_combo(");
    let known_exchange = braced_body(combo, "if let Some(venue) = venue");
    let unknown_exchange = braced_body(combo, "} else {");
    assert!(
        known_exchange.contains("MoonMenuItem::action_label(")
            && known_exchange.contains("let exchange = venue.id;")
            && known_exchange.contains("this.set_source(LogSource::Exchange(exchange), cx);")
            && !known_exchange.contains("MoonMenuItem::label(exchange_label)")
            && unknown_exchange.contains("MoonMenuItem::label(exchange_label)")
            && !unknown_exchange.contains("MoonMenuItem::action_label("),
        "known Log exchange headers must select an exchange source while Unknown stays passive"
    );

    let panel = read_src("panels/log/mod.rs");
    let snapshot = braced_body(&panel, "fn snapshot(");
    let membership = braced_body(&panel, "fn resolve_membership(");
    let pull = braced_body(&panel, "fn pull_rows(");
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
        panel.contains("Exchange(ExchangeId)")
            && snapshot.contains("LogSource::Exchange(_)")
            && snapshot.contains("exchange_membership")
            && arms(&panel) == 1
            && arms(&view) == 1
            && membership.contains("render::exchange_core_ids(")
            // A membership change cannot be appended to: rows written by a departed core would stay
            // under the selected exchange's label. Only a full reload drops them.
            && pull.contains("exchange_membership_changed(")
            && pull.contains("self.reload_rows(b, cx);")
            && reload.contains("self.cursors.clear();"),
        "the selected Log exchange source must read only its current live membership"
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

/// An open Log tab must extend its buffer, never rebuild it.
///
/// This is a cost contract, and it is invisible to every other kind of check: a full rebuild
/// produces exactly the same rows, so the panel looks correct while an open tab re-reads and
/// re-parses its whole source on every backend revision. Measured here at ten cores that was
/// ~25 ms per revision at 4 Hz — about a tenth of a core — against ~0.3 ms for the append path,
/// and it cost the same whether the errors-only filter kept a thousand rows or none.
///
/// The plausible regressions are a one-line "just reload, it's simpler" in `view.rs`, reading the
/// retained Classic source after effective Auto scope was resolved, or a parse creeping back into
/// the filter pass. All are shaped exactly like code that existed before the incremental path.
#[test]
fn an_open_log_tab_appends_new_lines_instead_of_rebuilding() {
    let view = read_src("panels/log/view.rs");
    let render_fn = braced_body(&view, "fn render(");
    assert!(
        render_fn.contains("self.pull_rows(backend.read(cx), cx);")
            && !render_fn.contains("self.reload_rows("),
        "an observed revision must take the incremental path, not a full reload"
    );

    let panel = read_src("panels/log/mod.rs");
    let pull = braced_body(&panel, "fn pull_rows(");
    let coherent_anchors = [
        "let (source, file, workspace_owned) = self.effective_selection(b);",
        "let membership = self.resolve_membership(b, &source);",
        "let sources = self.data_sources(b, &source, workspace_owned);",
        ".record_reload(render::log_sig(b, &self.group, &source));",
        "let fresh = self.cursors.pull(b, &source, &sources, membership.as_ref());",
        "self.append_rows(fresh, cx);",
    ];
    let anchor_positions = coherent_anchors.map(|anchor| {
        pull.find(anchor)
            .unwrap_or_else(|| panic!("the incremental Log path must contain `{anchor}`"))
    });
    assert!(
        anchor_positions.windows(2).all(|pair| pair[0] < pair[1])
            && !pull.contains("self.snapshot("),
        "the incremental path must snapshot one effective scope, record its signature, pull its \
         cursors, and append without rebuilding"
    );

    // Parsing belongs to arrival. The filter passes run over buffered rows — `refilter` over the
    // whole buffer on every keystroke, `passes` once per row — and must only READ what arrival
    // already computed. Naming the parsing calls rather than grepping for `to_lowercase` is
    // deliberate: an earlier spelling of this check passed the moment the lowering moved one
    // function along, which is placement, not the property.
    let buffer = read_src("panels/log/buffer.rs");
    let parsing = ["LineView::parse", "flatten_lines", "find_coin", "classify_"];
    for scope in ["fn refilter(", "fn passes(", "fn extend_view_from("] {
        let body = braced_body(&buffer, scope);
        assert!(
            parsing.iter().all(|call| !body.contains(call)),
            "{scope} must not re-parse buffered rows"
        );
    }
    assert!(
        braced_body(&buffer, "fn ingest(").contains("LineView::parse"),
        "new lines are the only ones that get parsed"
    );

    // The steady state of a busy source is a buffer sitting AT its cap, so eviction happens on
    // every revision. Rebasing the visible indices has to stay arithmetic; re-running the filters
    // over the whole buffer there would put the full pass back on the per-revision path by the
    // back door, with every other check here still green.
    let evict = braced_body(&buffer, "fn evict(");
    assert!(
        !evict.contains("refilter") && evict.contains("-= dropped"),
        "eviction must rebase the visible list, not refilter it"
    );

    // Splicing rows in above what is on screen moves the positions a selection is stored as. The
    // buffer reports that; dropping the report is how a held selection silently starts addressing
    // lines the user never picked.
    assert!(
        braced_body(&buffer, "fn ingest(").contains("Disturbance::Moved")
            && braced_body(&panel, "fn append_rows(").contains("self.selection.clear()"),
        "movement under a selection must reach the selection"
    );
}

/// The Log panel's cost must stay visible in `render_diag.log`.
///
/// This work is invisible to every other signal: the panel does it inside `render`, so a regression
/// shows up only as process CPU mixed in with the charts — which is exactly why the original defect
/// survived. Each counter has to sit at the site that does the work it names, or the number drifts
/// away from the thing it is trusted to measure and the log starts lying with confidence.
#[test]
fn the_log_panels_work_is_counted_where_it_happens() {
    let diag = read_src("diag.rs");
    // Inside the macro invocation, not merely somewhere in the file: a counter declared as a bare
    // `static` would still read as present here while `snapshot_and_reset` never samples or clears
    // it, so it would print nothing and silently accumulate.
    let declared = diag
        .split_once("diag_counters!(")
        .and_then(|(_, rest)| rest.split_once("\n);"))
        .map(|(list, _)| list)
        .expect("the counter list must stay a single macro invocation");
    for counter in [
        "LOG_RENDER",
        "LOG_PULL",
        "LOG_LINES_PARSED",
        "LOG_ROWS_FILTERED",
        "LOG_ROWS_EVICTED",
        "LOG_REFILTER",
        "LOG_RELOAD",
        "LOG_WORK_US",
    ] {
        assert!(
            declared.contains(counter),
            "{counter} must be declared inside diag_counters!, or it is never sampled"
        );
    }

    let buffer = read_src("panels/log/buffer.rs");
    let panel = read_src("panels/log/mod.rs");
    let view = read_src("panels/log/view.rs");

    // The render rate, as every sibling panel reports it. Without it a per-frame cost reads as a
    // per-revision one.
    assert!(
        braced_body(&view, "fn render(").contains("LOG_RENDER"),
        "the Log panel must report its render rate like Orders, Assets and News do"
    );

    // Volumes must be summed, not counted per call: `bump` here instead of `bump_by` would turn
    // "lines parsed" into "batches parsed" and hide a whole-buffer re-parse behind a rate of 4.
    for (scope, source, counter) in [
        ("fn ingest(", &buffer, "LOG_LINES_PARSED"),
        ("fn extend_view_from(", &buffer, "LOG_ROWS_FILTERED"),
        ("fn evict(", &buffer, "LOG_ROWS_EVICTED"),
    ] {
        let body = braced_body(source, scope);
        // Spelled as two independent facts rather than one exact call string: rustfmt reflows a long
        // call across lines, and a test pinned to one layout would go green by accident.
        assert!(
            body.contains(counter)
                && body.contains("bump_by(")
                && !body.contains(&format!("bump(&crate::diag::{counter}")),
            "{scope} must add its VOLUME to {counter}, not one event per call"
        );
    }

    // Whole-buffer filter passes and full re-reads are events, counted where they happen.
    assert!(
        braced_body(&buffer, "fn refilter(")
            .contains("crate::diag::bump(&crate::diag::LOG_REFILTER")
            && braced_body(&panel, "fn reload_rows(")
                .contains("crate::diag::bump(&crate::diag::LOG_RELOAD"),
        "whole passes and full re-reads must be counted at their own sites"
    );

    // Eviction must count what it DROPPED. Counting the rows that stayed would sit at the cap
    // forever — a busy panel is always at its cap — and print the correct state as a fault.
    let evict = braced_body(&buffer, "fn evict(");
    assert!(
        evict.contains("LOG_ROWS_EVICTED, dropped as u64")
            && !evict.contains("cap + self.view.len()"),
        "eviction must count dropped rows, not the buffer that survived"
    );

    // The revision path is timed at both ends of it, or the figure silently excludes the expensive
    // half: a full re-read is where the old whole-source cost lived.
    for scope in ["fn pull_rows(", "fn reload_rows("] {
        let body = braced_body(&panel, scope);
        assert!(
            body.contains("diag_timer()") && body.contains("record_work_us(timer)"),
            "{scope} must be timed into log_work_us"
        );
    }

    // `pull_rows` returns early into `reload_rows` for the cases a buffer cannot absorb. The bump
    // has to sit AFTER those, or reloads are reported as incremental reads — the one confusion that
    // would make the whole set say the opposite of the truth.
    let pull = braced_body(&panel, "fn pull_rows(");
    let bump_at = pull
        .find("LOG_PULL")
        .expect("the incremental read must be counted");
    let last_reload = pull
        .rfind("self.reload_rows(b, cx);")
        .expect("pull_rows must still fall back to a full reload");
    assert!(
        bump_at > last_reload,
        "LOG_PULL must be bumped after the reload fallbacks, not on every call"
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

/// `controls/mod.rs:lev_bounds_for` must preserve the terminal fallback only for an unknown cap
/// and otherwise end at the coin cap; restoring an account-current maximum lets users choose
/// leverage the exchange rejects.
#[test]
fn leverage_slider_bounds_end_at_the_known_coin_maximum() {
    let controls = read_src("controls/mod.rs");
    let bounds = code_only(fn_body(&controls, "pub fn lev_bounds_for("));

    assert!(
        bounds.contains("if coin_max <= 0 {\n        return LEV_BOUNDS;\n    }"),
        "an unknown or spot coin must retain the exact terminal fallback bounds"
    );
    assert!(
        bounds.contains("(min, coin_max as f32, step)"),
        "a known coin maximum must be the slider upper bound"
    );
    assert!(
        !bounds.contains(".max("),
        "the upper bound must not grow to preserve an above-cap current leverage"
    );
}

/// `controls/metric.rs:metric_popup_content` must keep its only `set_leverage` call in the Apply
/// button; adding one to a preset makes a single x50 click change real account leverage without
/// confirmation.
#[test]
fn leverage_presets_only_stage_values_until_apply() {
    let metric = code_only(braced_body(
        &read_src("controls/metric.rs"),
        "pub fn metric_popup_content(",
    ));
    assert_eq!(
        metric.matches("b.session.set_leverage(").count(),
        1,
        "metric_popup_content must contain exactly one exchange leverage write"
    );
    let apply = chain_between(
        &metric,
        "MoonButton::new(\"toolbar-lev-apply\")",
        ".render(),",
        "leverage Apply button",
    );
    assert!(
        apply.contains("b.session.set_leverage("),
        "the only exchange leverage write must remain inside toolbar-lev-apply"
    );
}

/// `controls/toolbar.rs:row_fit` must budget the Profit Monitor launcher; changing its fixed icon
/// multiplier back to four makes this assertion red and lets the trailing launcher clip at narrow
/// Main-window widths.
#[test]
fn toolbar_budget_includes_every_singleton_launcher() {
    let text = read_src("controls/toolbar.rs");
    let budget = fn_body(&text, "fn row_fit(");
    let toolbar = fn_body(&text, "pub fn toolbar(");
    let drawn = toolbar.matches("open_window_button(").count();
    assert_eq!(
        drawn, 5,
        "row_fit must budget every open_window_button rendered by toolbar"
    );
    assert!(
        budget.contains("ICON_BTN_W * 5.0"),
        "row_fit must reserve icon width for every open_window_button rendered by toolbar"
    );
    assert!(toolbar.contains("\"toolbar-profit-monitor\""));
    assert!(toolbar.contains("crate::analytics::profit_monitor::open"));
}

/// `controls/toolbar.rs:toolbar` must retain the requested launcher order and the two
/// semantic dividers: after Screener, then after Analytics. Moving any launcher or divider
/// changes the operator's stable target sequence even though every destination still opens.
#[test]
fn toolbar_orders_launchers_around_one_semantic_divider() {
    let text = read_src("controls/toolbar.rs");
    let toolbar = code_only(fn_body(&text, "pub fn toolbar("));
    let ids = [
        "toolbar-profit-monitor",
        "toolbar-screener",
        "toolbar-strategies",
        "toolbar-analytics",
        "toolbar-settings",
    ];
    let positions = ids.map(|id| {
        toolbar
            .find(&format!("\"{id}\""))
            .unwrap_or_else(|| panic!("{id} launcher must remain present"))
    });

    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "launcher order must be Profit Monitor, Screener, Strategies, Analytics, Settings"
    );
    let between_first_and_last = &toolbar[positions[0]..positions[4]];
    assert!(
        !toolbar[positions[0]..positions[1]].contains(".child(design::chrome_divider(cx, p))"),
        "Profit Monitor and Screener must share the leading launcher section"
    );
    assert!(
        !toolbar[positions[2]..positions[3]].contains(".child(design::chrome_divider(cx, p))"),
        "Strategies and Analytics must share one section"
    );
    assert_eq!(
        between_first_and_last
            .matches(".child(design::chrome_divider(cx, p))")
            .count(),
        2,
        "the trailing cluster must contain two internal dividers"
    );
    assert!(
        toolbar[positions[1]..positions[2]].contains(".child(design::chrome_divider(cx, p))"),
        "the first trailing divider must sit between Screener and Strategies"
    );
    assert!(
        toolbar[positions[3]..positions[4]].contains(".child(design::chrome_divider(cx, p))"),
        "the second trailing divider must sit between Analytics and Settings"
    );
}

/// `controls/toolbar.rs:row_fit` must derive all three optional launcher widths from their live
/// localized labels, while `open_window_button` must emit either the whole label or only a tooltip.
/// Reintroducing a fixed width or an always-present text segment clips translations at font scale.
#[test]
fn toolbar_launcher_labels_are_measured_and_all_or_none() {
    let text = read_src("controls/toolbar.rs");
    let fit = fn_body(&text, "fn row_fit(");
    let measure = fn_body(&text, "fn launcher_label_width(");
    let toolbar = fn_body(&text, "pub fn toolbar(");
    let button = fn_body(&text, "fn open_window_button(");

    assert!(measure.contains("design::ui_text_width("));
    assert!(measure.contains("TOOLBAR_LAUNCHER_TEXT_SIZE"));
    assert!(measure.contains("TOOLBAR_LAUNCHER_TEXT_WEIGHT"));
    assert!(
        measure.contains("true,"),
        "launcher widths must use the monospaced family inherited from the Shell root"
    );
    assert!(measure.contains(".max(ICON_BTN_W)"));
    for (label, width) in [
        ("launchers.analytics", "fit.analytics_width"),
        ("launchers.strategies", "fit.strategies_width"),
        ("launchers.settings", "fit.settings_width"),
    ] {
        assert!(
            fit.contains(&format!("launcher_label_width(cx, {label})")),
            "row_fit must measure {label}"
        );
        assert!(
            toolbar.contains(width),
            "toolbar must pass {width} to open_window_button"
        );
    }
    assert!(!text.contains("SETTINGS_BTN_W"));
    assert!(measure.contains("TOOLBAR_LAUNCHER_PAD_X"));
    assert!(button.contains("if labeled_width.is_some()"));
    assert!(button.contains("padding_x(TOOLBAR_LAUNCHER_PAD_X)"));
    assert!(button.contains("btn.text_segment(label") || button.contains(".text_segment(label"));
    assert!(button.contains("btn.tooltip(label)"));
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

/// Protects one-server Auto search from repeating the same server on every result row.
///
/// The plausible edit is changing `when(show_server_per_row, ..)` to `when(true, ..)` while
/// restyling the row. The popup would still show the correct Auto context above the list, but every
/// row would regain the redundant `@server` suffix and recreate the clutter this context removes.
#[test]
fn single_server_auto_search_names_the_server_once() {
    let coin_search = read_src("controls/coin_search.rs");
    let context = code_only(braced_body(
        &coin_search,
        "pub(crate) fn single_server_context(",
    ));
    assert!(
        context.contains("let [core] = cores.as_slice()"),
        "the popup context must require the actual search scope to resolve to exactly one core"
    );

    let popup = code_only(braced_body(
        &coin_search,
        "pub(crate) fn render_popup<F, G, H>(",
    ));
    assert!(
        popup.contains("let show_server_per_row = server_context.is_none()")
            && popup.contains("chart.coin.server_context"),
        "a popup-level server context must be the sole decision that suppresses row attribution"
    );
    let rows = code_only(braced_body(&coin_search, "fn push_section<F, G>("));
    assert!(
        rows.contains(".when(show_server_per_row, |row|"),
        "single-server Auto rows must omit the repeated visible @server suffix"
    );
    assert!(
        rows.contains("format!(\"{pair} @ {server}\")"),
        "the full instrument/server identity must remain available in the row tooltip"
    );

    let strip = code_only(braced_body(
        &read_src("chart_tabs/strip.rs"),
        "fn render(&mut self, window: &mut Window",
    ));
    assert!(
        strip.contains("auto_workspace_chart_core")
            && strip.contains("coin_search_bucket")
            && strip.contains("single_server_context")
            && strip.contains("server_context,"),
        "ChartTabs must Auto-gate the context and derive it from the active search bucket"
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

/// Protects every core-settings write handler from bypassing its seeded-target guard.
///
/// `shell/core_settings.rs::resolve_core_settings_write` is a pure decision the unit tests in
/// `shell/core_settings/tests.rs` exercise directly, but a call SITE reverted to a bare
/// `b.active_trade_core(&group)` compiles and is invisible from there — the pure function stays
/// green while the popup writes to whatever core is active at commit time instead of the one it
/// was seeded from. This pins every call site instead.
///
/// Comments are stripped first: the doc comments beside these guards name
/// `resolve_core_settings_write` in prose, so a raw substring search would stay green with the
/// call itself deleted.
#[test]
fn core_settings_writes_all_go_through_the_seeded_target_guard() {
    let popup = code_only(&read_src("shell/core_settings_popup.rs"));
    let reads = popup.matches("active_trade_core(&group)").count();
    let guarded = popup.matches("resolve_core_settings_write(seeded").count();
    assert!(
        reads > 0,
        "expected at least one active-core read in core_settings_popup.rs"
    );
    assert_eq!(
        guarded, reads,
        "every core_settings_popup.rs handler reading the active core must pass it through \
         resolve_core_settings_write(seeded, ..); found {reads} active-core reads but only \
         {guarded} guarded by the seeded-target check"
    );

    let core_settings = code_only(&read_src("shell/core_settings.rs"));
    for signature in [
        "pub(super) fn reconcile_core_settings_popup(",
        "pub(super) fn commit_blacklist_text(",
        "pub(super) fn core_settings_cancel_all_click(",
    ] {
        let body = braced_body(&core_settings, signature);
        assert!(
            body.contains("resolve_core_settings_write("),
            "{signature} must resolve its write address through resolve_core_settings_write"
        );
    }

    let metrics = code_only(&read_src("shell/metrics.rs"));
    let commit = braced_body(&metrics, "pub(super) fn commit_client_edit(");
    assert!(
        commit.contains("core_settings::resolve_core_settings_write("),
        "commit_client_edit must resolve its write address through resolve_core_settings_write, \
         not a bare b.active_trade_core(&self.group)"
    );
}

/// A table host must sit in a container that actually gives it HEIGHT.
///
/// This shipped broken twice in one sitting, both times invisible to every other gate: the build,
/// the unit tests, clippy and FireTest all pass while the panel draws a row counter and no table at
/// all — not even a header — because `MoonDataTable` is a virtual list with no content height of
/// its own. Two ways to lose it, and this panel found both:
///
/// - a bare `div()` is `Display::Block` (gpui `Style::default`), where the host's `flex_1` is inert;
/// - `h_flex()` is `flex_row().items_center()`, and a centred child is laid out at its content
///   height, which for that list is zero.
///
/// In a COLUMN, `flex_1` is the height — so the rule is that the table's container is a `v_flex`
/// carrying `flex_1`, and that the render lays nothing out with `h_flex`. Reverting either half
/// must fail here rather than in a screenshot.
#[test]
fn the_alerts_table_sits_in_a_container_that_gives_it_height() {
    let src = read_src("panels/alerts/mod.rs");
    let render = code_only(braced_body(
        &src,
        "fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement",
    ));
    assert!(
        render.contains("self.table(p, cx)"),
        "the Alerts render no longer builds its table here; move this contract with it"
    );
    let container = render
        .split_once("self.table(p, cx)")
        .expect("checked above")
        .0;
    let container = container
        .rsplit_once("let body = ")
        .unwrap_or_else(|| panic!("the Alerts table must be built into a `body` container"))
        .1;
    assert!(
        container.starts_with("v_flex()"),
        "the Alerts table's container must be a `v_flex`, where `flex_1` is the HEIGHT; it is: {}",
        container.lines().next().unwrap_or_default()
    );
    assert!(
        container.contains(".flex_1()"),
        "the Alerts table's container must claim the remaining height with `flex_1()`"
    );
    assert!(
        !render.contains("h_flex()"),
        "the Alerts render must not lay its body out with `h_flex()`: it centres its children, and          a centred virtual list is drawn at zero height"
    );
}

/// Protects the two menus from silently returning to their fixed 220-pixel widths.
///
/// The plausible production edit is replacing the fitted calls in `controls/coin_menu.rs` and
/// `panels/orders/controls.rs` with their former fixed-width APIs. It compiles and preserves every
/// action, but long core names and translated settings rows clip as in the reported screenshots.
#[test]
fn coin_and_orders_settings_menus_keep_fitted_width_routes() {
    let coin_menu = code_only(&read_src("controls/coin_menu.rs"));
    let orders = code_only(&read_src("panels/orders/controls.rs"));

    assert!(
        coin_menu.contains("window.open_fitted_moon_context_menu(")
            && coin_menu.contains("MENU_MIN_WIDTH,")
            && coin_menu.contains("MENU_MAX_WIDTH,"),
        "the shared coin menu must use the fitted Root-owned MoonUI context-menu route"
    );
    assert!(
        !coin_menu.contains("window.open_moon_context_menu("),
        "the shared coin menu must not restore its fixed-width Root route"
    );
    assert!(
        orders.contains(".fit_menu_width(SETTINGS_MENU_MIN_W, SETTINGS_MENU_MAX_W)")
            && orders.contains(".items(Self::sort_menu_items(&view, self.view))"),
        "Orders settings must fit its existing item model without replacing its callbacks"
    );
    assert!(
        !orders.contains(".menu_width(220.0)"),
        "Orders settings must not restore the fixed width that clips translated rows"
    );
}

/// Protects the two guards that keep a bare-key binding from firing on something the user did not
/// press.
///
/// Caps Lock and a lone modifier reach a window as a change of modifier state, so both windows read
/// them through `MoonHotkeyModifierWatch`. Two edits break that quietly. Dropping `forget()` from
/// the activation observer lets the state a window is RE-TOLD when it regains focus read as a
/// press: Caps Lock flipped in another application, or Alt still held from the Alt+Tab that brought
/// the window back, would run a trading action nobody asked for. Dropping the capture-phase
/// `interrupt()` on mouse-down lets the modifier held for a chart gesture — Moonbot's own Ctrl+Left
/// order move — commit as a binding when the hand comes off it.
#[test]
fn bare_key_bindings_ignore_a_refocused_state_and_a_mouse_gesture() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let files = [
        ("shell/init.rs", root.join("shell").join("init.rs")),
        ("shell/render.rs", root.join("shell").join("render.rs")),
        (
            "detached_host/mod.rs",
            root.join("chart_tabs").join("detached_host").join("mod.rs"),
        ),
        (
            "detached_host/render.rs",
            root.join("chart_tabs")
                .join("detached_host")
                .join("render.rs"),
        ),
    ];
    for (name, path) in files {
        let source = fs::read_to_string(&path).unwrap();
        let observer = name.ends_with("init.rs") || name.ends_with("mod.rs");
        if observer {
            assert!(
                source.contains("modifier_watch.forget()"),
                "{name} must forget the keyboard state when its window loses focus"
            );
        } else {
            assert!(
                source.contains("modifier_watch.interrupt()"),
                "{name} must withdraw a lone-modifier tap when a mouse gesture starts"
            );
            // Through `window::input_hook`, not inline: `Window::on_mouse_event` belongs to the
            // paint phase and `render` runs a phase earlier, which is a debug assertion in the
            // fork and killed the UI-atlas capture build on its first frame. What must not change
            // is that the listener is a WINDOW-level one - the chart consumes its own presses, so
            // a listener on the root element never sees them.
            assert!(
                source.contains("window_mouse_hook(") && source.contains("&MouseDownEvent"),
                "{name} must take mouse-down at the window level, through window::input_hook"
            );
        }
    }
}
