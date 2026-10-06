//! The toolbar row composition.

use super::*;

/// The toolbar strip: an ordinary `Shell` child between the header and the dock, not a dock panel.
/// It reads size and exit state from the window group, plus leverage and manual strategy from the
/// active core. While a chart is hovered, manual-strategy applicability follows that chart's core
/// so the row describes the order target under the pointer.
///
/// `chrome_width` is the window width. The row's controls are all `flex_none`, so nothing shrinks:
/// its optional labels collapse against that width by an explicit priority ([`row_fit`]), because
/// the row would otherwise push the trailing window buttons off the edge.
///
/// Args:
///     backend: Shared terminal state and singleton-window registry.
///     group: Main-window group whose trading controls are rendered.
///     size_edit: Active manual-size editor text and cell index, when any.
///     size_input: Shared input state for the active size editor.
///     sell_edit: Active sell-percentage editor text and cell index, when any.
///     sell_input: Shared input state for the active sell editor.
///     shell: Owning shell entity receiving toolbar actions.
///     settings_hint_at: When the first-run settings hint was armed, if it still is. Passed BY
///         VALUE rather than read off `shell`, because this row is built from inside the shell's
///         own render -- reading that entity here panics as a re-entrant borrow.
///     metric_popup: Active trade metric and its popup contents, when open.
///     max_order: Exchange maximum-order readout for the active leverage target.
///     quote: Quote token displayed beside a present maximum-order value.
///     chrome_width: Available toolbar width in logical pixels.
///     cx: Application context used for state reads and rendering.
///
/// Returns:
///     The complete responsive trading toolbar row.
// `use<>` on the return type: the row holds no input lifetime, and saying so is what lets a caller
// build it inside a scope narrower than the tree it is added to — the diagnostic timer around this
// call in `shell::render` is exactly that. Without it Rust 2024 captures every argument lifetime.
#[allow(clippy::too_many_arguments)]
pub fn toolbar(
    backend: &Entity<Backend>,
    group: &str,
    size_edit: Option<(String, usize)>,
    size_input: &Entity<MoonInputState>,
    sell_edit: Option<(String, usize)>,
    sell_input: &Entity<MoonInputState>,
    shell: &Entity<Shell>,
    settings_hint_at: Option<std::time::Instant>,
    metric_popup: Option<(TradeMetric, AnyElement)>,
    max_order: MaxOrderReadout,
    quote: &str,
    chrome_width: f32,
    cx: &App,
) -> impl IntoElement + use<> {
    let phase_us = crate::diag::timer();
    // Which metric is open and what its popup contains are ONE fact — `Shell` derives both from the
    // same field — so they arrive as one value: passed separately, a caller could name an open
    // metric with no content and the row would light a button over an empty popover. The content
    // does not clone, so it goes to exactly one button.
    let (lev_popup, sl_popup, tp_popup) = match metric_popup {
        Some((TradeMetric::Lev, content)) => (Some(content), None, None),
        Some((TradeMetric::Sl, content)) => (None, Some(content), None),
        Some((TradeMetric::Tp, content)) => (None, None, Some(content)),
        None => (None, None, None),
    };
    let manual_core = effective_manual_strategy_core(backend, group, cx);
    // The core whose manual config governs the toolbar's sizes and exits, independent of
    // manual-strategy applicability above: a chart can be addressed for one without being
    // addressed for the other, and neither gate may be built from the other's result — see
    // `chart_display_core`'s doc.
    let display_core = effective_chart_display_core(backend, group, cx);
    let (
        follow,
        overview,
        focus_core,
        write_matches_display,
        size_values,
        size_sel,
        size_source,
        core_config_edit,
        tp_value,
        tp_engaged,
        sl_value,
        sl_present,
        sl_on,
        lev_value,
        sell_pcts,
        sell_slot,
        manual_on,
        sl_locked,
        lev_unset_tip,
    ) = {
        let b = backend.read(cx);
        // Whether the header scope names one account at all. Read ONCE here so the leverage
        // button, the max-order readout and this row's own core cannot reach three different
        // conclusions about the same scope; the leverage ADDRESS is gated one level down, in
        // `TradeMetric`, so `Shell`'s open-popup guard shares the decision rather than copying it.
        let overview = b.is_auto_overview_scope(group);
        // Group-local size and exit controls do not move with this selection. Leverage reads a
        // core only when the visible scope names one; manual strategy still uses the active trade
        // core. In Overview, `active_trade_core` would answer with the group's first core and the
        // row would present that server's leverage as the group's.
        let focus_core = if overview {
            None
        } else {
            b.active_trade_core(group)
        };
        // Whether a click, double-click, or Ctrl+wheel on the size/sell strips right now would
        // write to the same source this row just displayed. `display_core` is hover-aware while
        // the write always targets `manual_write_core` (-> `active_trade_core`, never hover-aware),
        // so hovering a different chart in the same group can make the two disagree while the
        // strip still renders as live. `false` here disables both strips below (goal A2 FIX-3).
        let write_matches_display = b.manual_display_matches_write(group, display_core);
        // Sizes: [`Backend::effective_order_size_state`] is the one resolver every manual-trading
        // reader shares, so the row cannot reach a different "which core, which source" conclusion
        // than the choke point every write goes through. Both of its sources are local config this
        // terminal owns, so there is no freshness to report and nothing to wait for.
        let (size_values, size_sel, size_source) =
            b.effective_order_size_state(group, display_core);
        // Exits (TP/SL/sell presets): the exit twin of the sizes resolver above, resolved through
        // the same per-core-or-group choice so the two halves cannot disagree about the source.
        let (exit, _exit_source) = b.effective_group_exit(group, display_core);
        // The display core's one retained core-config write attempt, for the rejection notice
        // below. Read from the DISPLAY core so the tooltip describes the same core the row shows.
        let core_config_edit = display_core
            .and_then(|core| b.session.store().core(core))
            .and_then(|data| data.core_config_edit.clone());
        // Manual-strategy mode, and what it does to this row's exits — Moonbot's own arrangement:
        //
        // * the strategy owns the sell price and the stop, so both READOUTS come from it;
        // * the TP popup (slider and field) is closed in this mode: a free-form take profit has
        //   nowhere to go, since the strategy's sell price is a single value;
        // * the S presets are the ONE way to change that sell price, and only while Moonbot's
        //   "ignore the manual strategy's sell price" checkbox is on — with it off the strategy
        //   alone decides and the strip is disabled;
        // * the SL button and its toggle follow the same core's "Moonbot logic" switch: with it on
        //   — the default — the stop is the strategy's and both controls only report it; with it
        //   off they are editable and the visible stop is written to the order once it is placed.
        // Locked only while the core sells a manual order at the STRATEGY's own price: there the
        // terminal's TP and S presets reach nothing. With Moonbot's "ignore the manual strategy's
        // sell price" checkbox on they are ordinary controls again — their value rides along with
        // the order as `planned_sell_price` — so they stay live, slider and hotkeys included.
        let manual_on = manual_core
            .map(|c| {
                b.manual_strat_active(c).is_some() && !b.ignore_strat_sell_price(c).unwrap_or(false)
            })
            .unwrap_or(false);
        // The TP button always shows its own `take_profit_pct`, even while an S slot is engaged;
        // selecting a slot must not replace the value displayed by TP.
        // In manual-strategy mode both readouts come from the STRATEGY, because that is what the
        // core will use for the order; the group's own values return the moment MS goes off.
        // While a manual strategy owns the exits, BOTH readouts come from its overlay; with MS
        // off — or on another chart — they are the saved generation, untouched underneath.
        let manual_exit = manual_core.and_then(|c| b.manual_exit_overlay(c));
        let tp_value = format!(
            "{}%",
            fmt_field2(
                manual_exit
                    .and_then(|ms| ms.take_profit_pct)
                    .unwrap_or(exit.take_profit_pct) as f32
            )
        );
        let tp_engaged = exit.fixed_sell_slot.is_none() && !manual_on;
        // Who owns the stop, decided once and used for the value, the toggle and the lock alike —
        // three readings of one fact that must not diverge. Moonbot's own rule is on by default per
        // core; the switch is in the MS gear popup beside the toggle that turns the mode on.
        let manual_stop = manual_core
            .map(|c| b.manual_stop(c))
            .unwrap_or(ManualStop::Free);
        let sl_locked = manual_stop.locked();
        // SL is signed: `+1.00%` / `-20.00%`, avoiding `--` from manually prefixing a negative
        // value. The strategy stores its stop as a positive distance, so it is negated on the way
        // in and this reads the way the toolbar's own stop does. A DASH where the strategy owns the
        // stop and its value cannot be read: the saved generation would look like an answer here,
        // and it is not the number the order carries.
        let sl_on = manual_stop.stop_on(
            manual_exit
                .map(|ms| ms.stop_on)
                .unwrap_or(exit.stop_loss_enabled),
        );
        // Threaded alongside `sl_value` rather than re-derived from it below, so the readout's
        // colour and tooltip cannot disagree with the dash by matching different text later.
        let sl_present = !matches!(manual_stop, ManualStop::Unknown);
        let sl_value = match manual_stop {
            ManualStop::Strategy { pct, .. } => format!("{}%", fmt_field2_signed(pct)),
            ManualStop::Unknown => DASH.to_string(),
            ManualStop::Free => format!(
                "{}%",
                fmt_field2_signed(
                    manual_exit
                        .map(|ms| ms.stop_pct)
                        .unwrap_or(exit.stop_loss_pct)
                )
            ),
        };
        let sell_pcts = exit.fixed_sell_pcts;
        let sell_slot = exit.fixed_sell_slot;
        // Leverage is the Main chart market's per-core, per-market value from assets.
        // `seed_value` rather than `current`: it carries the same "0 means not set" rule the
        // popup's own open guard uses, so the dash and the disabled button cannot disagree with it.
        let lev_value = TradeMetric::Lev
            .seed_value(b, group)
            .map(|l| format!("×{}", l as i32));
        let lev_unset_tip = TradeMetric::Lev
            .unset_tooltip_key(b, group)
            .map(|key| t!(key).to_string());
        // The target chart wins over the header selection while it is hovered: mouse and market
        // hotkeys address that chart's independent core, whose manual strategy can override the
        // visible group exit values.
        (
            b.follow,
            overview,
            focus_core,
            write_matches_display,
            size_values,
            size_sel,
            size_source,
            core_config_edit,
            tp_value,
            tp_engaged,
            sl_value,
            sl_present,
            sl_on,
            lev_value,
            sell_pcts,
            sell_slot,
            manual_on,
            sl_locked,
            lev_unset_tip,
        )
    };
    let p = MoonPalette::active(cx);
    // `p.blue` in both themes, not the light theme's `p.accent`: the light Blue button uses
    // `p.accent` as its opaque fill, so repeating that token for the TP segment would merge the
    // percentage into its selected background.
    let tp_color = p.blue;
    let sl_color = design::danger_color(p);

    // Whether each metric can be edited at all. Derived through `TradeMetric` from the state this
    // block already read, so the row and the `Shell` state that outlives an open popup cannot come
    // to different conclusions about the same metric.
    //
    // Leverage alone needs a live value. Group-owned TP and SL are complete even before a core
    // connects, so their availability depends only on manual-strategy and SL-toggle state.
    let has_core = focus_core.is_some();
    let lev_available = TradeMetric::Lev.available_with(has_core, sl_on, manual_on, sl_locked)
        && lev_value.is_some();
    let tp_available = TradeMetric::Tp.available_with(has_core, sl_on, manual_on, sl_locked);
    let sl_available = TradeMetric::Sl.available_with(has_core, sl_on, manual_on, sl_locked);
    let lev_present = lev_value.is_some();
    let lev_str = lev_value.unwrap_or_else(|| "—".to_string());
    let tp_tip = (!manual_on).then(|| {
        if tp_engaged {
            t!("toolbar.tp_main_active_hint").to_string()
        } else {
            t!(
                "toolbar.tp_fixed_active_hint",
                n = sell_slot.unwrap_or_default()
            )
            .to_string()
        }
    });
    // Unlike Lev (`lev_unset_tip`) and MAX (`max_order_tip`), the SL dash used to carry no
    // explanation at all — muting it without saying why still leaves the operator guessing.
    let sl_unset_tip = (!sl_present).then(|| t!("toolbar.sl_unset_tip").to_string());

    // Cells are fitted BEFORE rendering: both the strip itself and the row budget that decides the
    // labels' fate read them. One computation, one source.
    crate::diag::record_us(&crate::diag::TOOLBAR_DATA_US, phase_us);
    let phase_us = crate::diag::timer();
    let size_cells = strips::FittedCells::fit(cx, strips::size_labels(size_values));
    let sell_cells = strips::FittedCells::fit(cx, strips::sell_labels(sell_pcts));
    // Two whole-block notices, in priority order. The strips going non-interactive comes first: it
    // explains why nothing on this row can be clicked (goal A2 FIX-3). A core-config rejection is
    // next — the numbers on the row are local config with nothing to be stale about, but the gear
    // popup's write to this core can still have been refused, and this tooltip is where that fact
    // surfaces.
    let manual_block_tip = (!write_matches_display)
        .then(|| SharedString::from(t!("toolbar.core_manual_mismatch").to_string()))
        .or_else(|| {
            core_config_rejection_caption(
                core_config_edit
                    .as_ref()
                    .and_then(|row| row.mismatches.as_ref()),
            )
        });
    // The exchange's own cap on a single order, kept permanently on the row rather than only inside
    // the leverage popover: it bounds every order the row above it composes, and a cap you have to
    // open a popup to see is a cap you check after sizing rather than before. Compact here, exact in
    // the popover — one value from one read, at two precisions.
    let max_order_caption = t!("toolbar.max_order_short").to_string();
    // A cap is one exchange account's rule. `Shell` resolved this readout through
    // `TradeMetric::Lev.target`, which already answers `None` in Overview — but `None` there means
    // `NoData`, whose hover text says the limits have not loaded yet. That is a false explanation
    // for a dash the scope caused, so the state is named explicitly instead: value and tooltip then
    // come from ONE value and cannot disagree about why the figure is absent.
    let max_order = if overview {
        MaxOrderReadout::OutOfScope
    } else {
        max_order
    };
    let max_order_value = max_order.format_compact(fmt::compact_si, quote);
    let max_order_tip = t!(max_order.tooltip_key()).to_string();
    let analytics_label = t!("toolbar.analytics").to_string();
    let strategies_label = t!("toolbar.strategies").to_string();
    let settings_label = t!("shell.settings_btn").to_string();
    let fit = row_fit(
        cx,
        chrome_width,
        &size_cells,
        &sell_cells,
        LauncherLabels {
            analytics: &analytics_label,
            strategies: &strategies_label,
            settings: &settings_label,
        },
        &max_order_caption,
        &max_order_value,
    );
    crate::diag::record_us(&crate::diag::TOOLBAR_FIT_US, phase_us);
    let phase_us = crate::diag::timer();

    // A section carries the gap INSIDE it; the boundary between two sections is drawn by the RULE
    // standing between them, not by a wider gap. Shared with the header — see
    // `design::chrome_section`.
    let section = || design::chrome_section(cx);

    let mut row = h_flex()
        .id("toolbar")
        // Anchors the overflow section when it pins itself to the right edge (`LauncherFold`).
        .relative()
        .w_full()
        .h(px(design::toolbar_height(cx)))
        .flex_none()
        .items_center()
        .gap(design::ui_px(cx, design::CHROME_GAP))
        .px(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(rgb(p.shell_high))
        // The bottom border bounds the whole chrome block (header + toolbar share one background)
        // against the transparent dock region below — it is not a seam between the two rows, which
        // is why the header carries none.
        .border_b_1()
        .border_color(rgb(p.border));

    row = row
        // §0 SOURCE. Leftmost, before everything it governs: this switch decides WHICH generation
        // of sizes and exits the rest of the row shows and edits, so it reads as the row's subject
        // rather than as one more control inside the size group.
        .children(own_trade_toggle(
            display_core,
            size_source == ManualSource::CoreOwn,
            backend,
            design::theme_installs_roles(cx),
        ))
        // §1 ORDER SIZE. Follows the switch: it is the quantity the other three sections modify —
        // leverage scales it, the stop bounds it, and TP/S define its target exit.
        .child(
            section().child(captioned_strip(
                "toolbar-size-caption",
                fit.size_caption,
                p,
                size_strip(
                    &size_cells,
                    size_sel,
                    // Show the editor only when the request belongs to this toolbar's group.
                    size_edit
                        .filter(|(edit_group, _)| edit_group == group)
                        .map(|(_, i)| i),
                    size_input,
                    backend.clone(),
                    // `None` disables the strip when the displayed core and the write target
                    // disagree (goal A2 FIX-3) — a live control must not mutate a source other
                    // than the one this row just showed.
                    write_matches_display.then(|| group.to_string()),
                    SIZE_UNIT,
                ),
                manual_block_tip.clone(),
                design::CHROME_GAP,
                cx,
            )),
        )
        // §2 LEVERAGE — its own section rather than an appendix to size: a leverage edit goes TO
        // THE EXCHANGE (`session.set_leverage`, behind an explicit Apply button in the popup),
        // whereas an order size is a local preset in the config. Different blast radius, different
        // group.
        .child(design::chrome_divider(cx, p))
        .child(
            section()
                .child(metric_button(
                    TradeMetric::Lev,
                    lev_str,
                    design::readout_color(p, lev_present),
                    design::font_w(cx, LEV_W),
                    lev_popup.is_some(),
                    false,
                    lev_available,
                    lev_unset_tip.map(SharedString::from),
                    lev_popup,
                    shell.clone(),
                    p,
                    cx,
                ))
                // The exchange max order joins THIS section rather than opening one of its own: it
                // constrains the same "how large, at what leverage" decision the section already
                // owns, and a section of its own would need a rule plus a root gap — the rule is
                // pinned by `toolbar_row_budget_counts_every_rule_it_draws` and the gap count is
                // guarded by nothing at all.
                .children(fit.max_order_caption.map(|text| strip_caption(text, p, cx)))
                // The VALUE is unconditional: this readout exists so the exchange's cap is on
                // screen while an order is being sized, and a figure that disappears on a narrow
                // window is not that. Only its caption yields to width — the tooltip below still
                // names the figure once the word is gone.
                .child(
                    div()
                        .id("toolbar-max-order")
                        .flex_none()
                        .child(strip_text(
                            max_order_value,
                            design::readout_color(p, max_order.value().is_some()),
                            cx,
                        ))
                        .tooltip(crate::panels::common::text_tooltip(max_order_tip)),
                ),
        )
        // §3 RISK: the on/off toggle (`panic_if_price_drop`) plus the value button and its popup.
        .child(design::chrome_divider(cx, p))
        .child(
            section()
                // Disabled only where the stop is not this terminal's to move: with a manual
                // strategy selected and Moonbot's own rule in force, this reports the strategy's
                // `UseStopLoss`. Turn that rule off in the MS popup and it is editable again.
                .child(sl_toggle(
                    sl_on,
                    sl_locked,
                    backend.clone(),
                    group.to_string(),
                    design::theme_installs_roles(cx),
                ))
                .child(metric_button(
                    TradeMetric::Sl,
                    sl_value,
                    if sl_present {
                        sl_color
                    } else {
                        design::readout_color(p, false)
                    },
                    design::font_w(cx, SL_W),
                    sl_popup.is_some(),
                    false,
                    sl_available,
                    sl_unset_tip.map(SharedString::from),
                    sl_popup,
                    shell.clone(),
                    p,
                    cx,
                )),
        )
        // §4 EXIT: TP and the S-slot strip are one and the same sell target, and exactly one of
        // them is lit. Hence ONE section: a rule between them would claim they are separate things.
        .child(design::chrome_divider(cx, p))
        .child({
            let strip = sell_strip(
                &sell_cells,
                sell_slot.filter(|_| !manual_on),
                // Show the S editor only when the request belongs to this toolbar's group.
                sell_edit
                    .filter(|(edit_group, _)| edit_group == group && !manual_on)
                    .map(|(_, i)| i),
                sell_input,
                backend.clone(),
                // Disabled only where a click would reach nothing: a manual strategy owning the
                // sell price. A displayed core disagreeing with the write target disables it too
                // (goal A2 FIX-3).
                (!manual_on && write_matches_display).then(|| group.to_string()),
            );
            let sell_block = captioned_strip(
                "toolbar-sell-caption",
                fit.sell_caption,
                p,
                strip,
                manual_block_tip.clone(),
                SELL_CAPTION_GAP,
                cx,
            );
            section()
                .child(metric_button(
                    TradeMetric::Tp,
                    tp_value,
                    tp_color,
                    design::font_w(cx, TP_W),
                    tp_popup.is_some(),
                    tp_engaged && !manual_on,
                    tp_available,
                    tp_tip.map(SharedString::from),
                    tp_popup,
                    shell.clone(),
                    p,
                    cx,
                ))
                .child(sell_block)
        });
    // Scale is configured per tab in the chart-tab strip beside the settings button; see
    // controls::scale_dropdown_for_tabs / chart_tabs::ChartTabs::pick_active_scale.

    let live_tone = if follow {
        design::positive_color(p)
    } else {
        p.text_muted
    };
    let live_label = if follow {
        t!("toolbar.live").to_string()
    } else {
        t!("toolbar.pause").to_string()
    };
    let backend_live = backend.clone();
    // §5 SESSION — Live is fenced off from the trading parameters to its left: it governs whether
    // the chart follows the market, not anything about an order.
    row = row.child(design::chrome_divider(cx, p)).child(
        section().child(
            MoonButton::new("live")
                .width(design::font_w(cx, LIVE_W))
                .variant(MoonButtonVariant::Soft)
                // Keep the localized interaction hint reachable without adding another row label.
                .tooltip(t!("toolbar.live_tip").to_string())
                .segment(
                    MoonButtonSegment::new("●")
                        .color(live_tone)
                        .font_size(9.0)
                        .weight(700.0),
                )
                .segment(
                    MoonButtonSegment::new(live_label)
                        .color(live_tone)
                        .weight(500.0),
                )
                .on_click(move |_, _, cx| {
                    backend_live.update(cx, |b, bcx| {
                        b.toggle_follow();
                        bcx.notify();
                    });
                })
                .render(),
        ),
    );
    crate::diag::record_us(&crate::diag::TOOLBAR_TRADE_US, phase_us);
    let phase_us = crate::diag::timer();
    // Trailing edge: Profit Monitor + Screener, then Strategies + Analytics, then Settings. A
    // launcher the row cannot hold folds into the overflow button at the very end (`row_fit`), so
    // none of them can be pushed past the window's edge.
    let folds = fit.launchers;
    let launch = |launcher: Launcher| -> LaunchTarget {
        match launcher {
            Launcher::ProfitMonitor => LaunchTarget {
                id: "toolbar-profit-monitor",
                label: t!("toolbar.profit_monitor").to_string(),
                icon: "icons/trending-up.svg",
                labeled_width: None,
                workspace_owner: None,
                open: crate::analytics::profit_monitor::open,
            },
            Launcher::Screener => LaunchTarget {
                id: "toolbar-screener",
                label: t!("toolbar.screener").to_string(),
                icon: "icons/chart-pie.svg",
                labeled_width: None,
                workspace_owner: None,
                open: crate::screener::open,
            },
            Launcher::Strategies => LaunchTarget {
                id: "toolbar-strategies",
                label: strategies_label.clone(),
                icon: super::STRATEGIES_ICON,
                labeled_width: fit.strategies_width,
                workspace_owner: Some(group.to_string()),
                open: crate::strategies::open,
            },
            Launcher::Analytics => LaunchTarget {
                id: "toolbar-analytics",
                label: analytics_label.clone(),
                icon: "icons/layout-dashboard.svg",
                labeled_width: fit.analytics_width,
                workspace_owner: Some(group.to_string()),
                open: crate::analytics::open,
            },
            Launcher::Settings => LaunchTarget {
                id: "toolbar-settings",
                label: settings_label.clone(),
                icon: "icons/settings.svg",
                labeled_width: fit.settings_width,
                workspace_owner: None,
                open: crate::settings::open,
            },
        }
    };
    let button = |launcher: Launcher| {
        folds
            .shows(launcher)
            .then(|| open_window_button(launch(launcher), backend.clone(), p, cx))
    };
    // The first-run hint. TWO conditions, not one: the timer decides how long the ring breathes,
    // and the saved config decides whether it is still relevant at all -- so the moment a core is
    // saved the ring is gone on the NEXT FRAME rather than at the end of its timer. Read from
    // `backend.config`, never from the Settings draft: an unsaved row the user is still typing
    // into is not a configured core. It rides whichever control opens Settings right now: the
    // Settings button, or the overflow button while Settings is folded into it.
    let settings_hint = settings_hint_at
        .filter(|_| !backend.read(cx).config.core_ever_configured())
        .and_then(|at| crate::pulse::attention_ring(p.accent, at));
    let (settings_ring, overflow_ring) = if folds.shows(Launcher::Settings) {
        (settings_hint, None)
    } else {
        (None, settings_hint)
    };
    let folded: Vec<LaunchTarget> = LAUNCHER_FOLD_ORDER[..folds.folded]
        .iter()
        .rev()
        .map(|&launcher| launch(launcher))
        .collect();
    let shows_any = |set: &[Launcher]| set.iter().any(|&l| folds.shows(l));
    let row = row
        .child(div().flex_1())
        .when(
            shows_any(&[Launcher::ProfitMonitor, Launcher::Screener]),
            |row| {
                row.child(design::chrome_divider(cx, p)).child(
                    section()
                        .children(button(Launcher::ProfitMonitor))
                        .children(button(Launcher::Screener)),
                )
            },
        )
        .when(
            shows_any(&[Launcher::Strategies, Launcher::Analytics]),
            |row| {
                row.child(design::chrome_divider(cx, p)).child(
                    section()
                        // Launcher captions are control captions, so they read in the UI face. It
                        // is set on the section rather than per button because `MoonButton` can
                        // only force MONO on its own segments -- it has no proportional prop, and
                        // inherits otherwise. Paired with `launcher_label_width`, which measures
                        // the same family.
                        .font_family(design::ui_font())
                        .children(button(Launcher::Strategies))
                        .children(button(Launcher::Analytics)),
                )
            },
        )
        // The Settings section always exists: it holds the Settings button, the overflow button,
        // or both — the overflow button takes the slot Settings vacates when it folds first.
        .child(design::chrome_divider(cx, p))
        .child(
            section()
                // The trading controls alone outgrow the window, so the end of the flow is past
                // its edge: the section (by then the overflow button alone) is laid over the
                // row's right edge instead, on the row's own background with a rule before it.
                .when(folds.pinned, |section| {
                    section
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right(design::ui_px(cx, design::HEADER_PAD_X))
                        .pl(design::ui_px(cx, design::CHROME_GAP))
                        .bg(rgb(p.shell_high))
                        .border_l_1()
                        .border_color(rgb(p.border))
                })
                .children(button(Launcher::Settings).map(|settings| {
                    div()
                        .relative()
                        // The UI face, as for the other labeled launchers above.
                        .font_family(design::ui_font())
                        .child(settings)
                        // Declared AFTER the button so the ring paints on top of its chrome; it
                        // is a pointer-transparent overlay and takes no clicks from the control
                        // beneath.
                        .children(settings_ring)
                }))
                .when(!folded.is_empty(), |section| {
                    section.child(
                        div()
                            .relative()
                            .child(overflow_button(folded, backend.clone(), p, cx))
                            .children(overflow_ring),
                    )
                }),
        );
    crate::diag::record_us(&crate::diag::TOOLBAR_LAUNCH_US, phase_us);
    row
}
