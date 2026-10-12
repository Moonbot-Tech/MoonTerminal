//! The toolbar row: its resolved state and its composition.

use super::*;

/// The pods' localized captions, resolved once per frame for both measurement and render.
pub(crate) struct PodCaptions {
    pub(crate) strategy: String,
    pub(crate) size: String,
    pub(crate) lev: String,
    pub(crate) sl: String,
    pub(crate) tp: String,
    pub(crate) sell: String,
}

impl PodCaptions {
    fn read() -> Self {
        Self {
            strategy: t!("toolbar.strategy_caption").to_string(),
            size: t!("toolbar.size_caption").to_string(),
            lev: t!("toolbar.lev_caption").to_string(),
            sl: t!("toolbar.sl_caption").to_string(),
            tp: t!("toolbar.tp_caption").to_string(),
            sell: t!("toolbar.sell_caption").to_string(),
        }
    }
}

/// Why the order controls are locked under an overlay.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ToolbarLock {
    /// The chart under the cursor belongs to another core than the one a write would reach.
    OtherCore(String),
    /// The group has no cores at all.
    NoCores,
}

/// Everything the toolbar shows, resolved once per frame.
///
/// Measurement ([`toolbar_widths`]) and render ([`render_toolbar`]) both read this one value, so
/// the counted and the drawn row cannot disagree about a single figure.
pub(crate) struct ToolbarModel {
    pub(crate) group: String,
    follow: bool,
    /// The core whose manual config the size and exit controls show.
    pub(crate) display_core: Option<CoreId>,
    /// Whether a click on the strips would write to the source this row displays.
    write_matches_display: bool,
    pub(crate) size_cells: strips::FittedCells,
    pub(crate) size_sel: usize,
    own_on: bool,
    pub(crate) sell_cells: strips::FittedCells,
    sell_slot: Option<usize>,
    /// A manual strategy owns the sell target: TP and the S presets reach nothing.
    manual_on: bool,
    pub(crate) tp_value: String,
    tp_engaged: bool,
    tp_available: bool,
    tp_tip: Option<String>,
    pub(crate) sl_value: String,
    sl_present: bool,
    sl_on: bool,
    sl_locked: bool,
    sl_available: bool,
    sl_unset_tip: Option<String>,
    pub(crate) lev_value: String,
    lev_present: bool,
    lev_available: bool,
    lev_unset_tip: Option<String>,
    /// `of ×max ·` when the exchange reports a leverage cap.
    pub(crate) lev_of: Option<String>,
    pub(crate) max_order_inline: String,
    pub(crate) max_order_value: String,
    max_order_present: bool,
    max_order_tip: String,
    /// No coin on the chart: one hint replaces the Leverage pod.
    pub(crate) no_coin: bool,
    pub(crate) no_coin_text: String,
    manual_block_tip: Option<SharedString>,
    pub(crate) lock: Option<ToolbarLock>,
    pub(crate) live_label: String,
    pub(crate) captions: PodCaptions,
    /// The Strategy pod — MANUAL workspace, not Overview, and a core with Manual-kind strategies.
    pub(crate) strat: Option<StratPod>,
}

/// Which order controls take an edit right now. One gate for a pod and the "More" row it folds
/// into, so a locked row is locked in both places.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OrderGates {
    pub(crate) take: bool,
    pub(crate) sl_toggle: bool,
    pub(crate) sl_value: bool,
    pub(crate) sell: bool,
    pub(crate) size: bool,
    pub(crate) own_set: bool,
}

impl ToolbarModel {
    /// The S slot the sell control shows as engaged: none while a manual strategy owns the target.
    pub(crate) fn sell_slot_shown(&self) -> Option<usize> {
        self.sell_slot.filter(|_| !self.manual_on)
    }

    /// Whether a write would reach the source this row displays — false under either lock.
    pub(crate) fn interactive(&self) -> bool {
        self.write_matches_display && self.lock.is_none()
    }

    /// The order controls' gates: each control's own availability, all of them closed while the
    /// row is locked.
    pub(crate) fn order_gates(&self) -> OrderGates {
        let interactive = self.interactive();
        OrderGates {
            take: interactive && self.tp_available,
            sl_toggle: interactive && !self.sl_locked,
            sl_value: interactive && self.sl_available && self.sl_on,
            sell: interactive && !self.manual_on,
            size: interactive,
            own_set: interactive && self.display_core.is_some(),
        }
    }

    /// Resolve the toolbar's state for `group`.
    ///
    /// Args:
    ///     backend: Shared terminal state.
    ///     group: Main-window group whose trading controls are rendered.
    ///     max_order: Exchange maximum-order readout for the active leverage target.
    ///     max_leverage: Exchange leverage cap for the same target, when the exchange reports one.
    ///     quote: Quote token displayed beside a present maximum-order value.
    ///     p: Active palette.
    ///     cx: Application context used for state reads and measurement.
    ///
    /// Returns:
    ///     The resolved model.
    pub(crate) fn read(
        backend: &Entity<Backend>,
        group: &str,
        max_order: MaxOrderReadout,
        max_leverage: Option<i32>,
        quote: &str,
        p: MoonPalette,
        cx: &App,
    ) -> Self {
        let manual_core = effective_manual_strategy_core(backend, group, cx);
        // The core whose manual config governs the toolbar's sizes and exits, independent of
        // manual-strategy applicability above: a chart can be addressed for one without being
        // addressed for the other, and neither gate may be built from the other's result — see
        // `chart_display_core`'s doc.
        let display_core = effective_chart_display_core(backend, group, cx);
        let b = backend.read(cx);
        // Whether the header scope names one account at all. Read ONCE here so the leverage
        // button, the max-order readout and the Strategy pod cannot reach three different
        // conclusions about the same scope; the leverage ADDRESS is gated one level down, in
        // `TradeMetric`, so `Shell`'s open-popup guard shares the decision rather than copying it.
        let overview = b.is_auto_overview_scope(group);
        // Group-local size and exit controls do not move with this selection. Leverage reads a
        // core only when the visible scope names one. In Overview, `active_trade_core` would
        // answer with the group's first core and the row would present that server's leverage as
        // the group's.
        let focus_core = if overview {
            None
        } else {
            b.active_trade_core(group)
        };
        // Whether a click, double-click, or Ctrl+wheel on the size/sell strips right now would
        // write to the same source this row just displayed. `display_core` is hover-aware while
        // the write always targets `manual_write_core` (-> `active_trade_core`, never hover-aware),
        // so hovering a different chart in the same group can make the two disagree while the
        // strip still renders as live. `false` here locks the order controls (goal A2 FIX-3).
        let write_matches_display = b.manual_display_matches_write(group, display_core);
        // Sizes: [`Backend::effective_order_size_state`] is the one resolver every manual-trading
        // reader shares, so the row cannot reach a different "which core, which source" conclusion
        // than the choke point every write goes through.
        let (size_values, size_sel, size_source) =
            b.effective_order_size_state(group, display_core);
        // Exits (TP/SL/sell presets): the exit twin of the sizes resolver above.
        let (exit, _exit_source) = b.effective_group_exit(group, display_core);
        // The display core's one retained core-config write attempt, for the rejection notice.
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
        // * the stop switch and value follow the same core's "Moonbot logic" switch: with it on
        //   — the default — the stop is the strategy's and both controls only report it; with it
        //   off they are editable and the visible stop is written to the order once it is placed.
        let manual_on = manual_core
            .map(|c| {
                b.manual_strat_active(c).is_some() && !b.ignore_strat_sell_price(c).unwrap_or(false)
            })
            .unwrap_or(false);
        // While a manual strategy owns the exits, BOTH readouts come from its overlay; with MS
        // off — or on another chart — they are the saved generation, untouched underneath.
        let manual_exit = manual_core.and_then(|c| b.manual_exit_overlay(c));
        // A take is a gain: signed like one, as the mockup's green `+1.00%`.
        let tp_value = format!(
            "+{}%",
            fmt_field2(
                manual_exit
                    .and_then(|ms| ms.take_profit_pct)
                    .unwrap_or(exit.take_profit_pct) as f32
            )
        );
        let tp_engaged = exit.fixed_sell_slot.is_none() && !manual_on;
        // Who owns the stop, decided once and used for the value, the switch and the lock alike —
        // three readings of one fact that must not diverge.
        let manual_stop = manual_core
            .map(|c| b.manual_stop(c))
            .unwrap_or(ManualStop::Free);
        let sl_locked = manual_stop.locked();
        let sl_on = manual_stop.stop_on(
            manual_exit
                .map(|ms| ms.stop_on)
                .unwrap_or(exit.stop_loss_enabled),
        );
        let sl_present = !matches!(manual_stop, ManualStop::Unknown);
        // SL is signed: `+1.00%` / `-20.00%`. A DASH where the strategy owns the stop and its
        // value cannot be read: the saved generation would look like an answer here, and it is not
        // the number the order carries.
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
        // Leverage is the Main chart market's per-core, per-market value from assets. `seed_value`
        // carries the same "0 means not set" rule the popup's own open guard uses.
        let lev_value = TradeMetric::Lev
            .seed_value(b, group)
            .map(|l| format!("×{}", l as i32));
        let lev_unset_key = TradeMetric::Lev.unset_tooltip_key(b, group);
        let own_on = size_source == ManualSource::CoreOwn;
        let cores: Vec<(CoreId, String)> = b.group_cores(group).into_iter().collect();
        let lock = if cores.is_empty() {
            Some(ToolbarLock::NoCores)
        } else if !write_matches_display {
            let name = display_core
                .and_then(|core| cores.iter().find(|(id, _)| *id == core))
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| DASH.to_string());
            Some(ToolbarLock::OtherCore(name))
        } else {
            None
        };
        let follow = b.follow;
        let strat = strat_pod_gate(b.workspace_mode(group), overview)
            .then(|| StratPod::read(group, backend, p, cx))
            .flatten();

        // Whether each metric can be edited at all, derived through `TradeMetric` so the row and
        // the `Shell` state that outlives an open popup cannot reach different conclusions.
        let has_core = focus_core.is_some();
        let lev_present = lev_value.is_some();
        let lev_available =
            TradeMetric::Lev.available_with(has_core, sl_on, manual_on, sl_locked) && lev_present;
        let tp_available = TradeMetric::Tp.available_with(has_core, sl_on, manual_on, sl_locked);
        let sl_available = TradeMetric::Sl.available_with(has_core, sl_on, manual_on, sl_locked);
        let tp_tip = (!manual_on).then(|| {
            if tp_engaged {
                t!("toolbar.tp_main_active_hint").to_string()
            } else {
                t!(
                    "toolbar.tp_fixed_active_hint",
                    n = exit.fixed_sell_slot.unwrap_or_default()
                )
                .to_string()
            }
        });
        let sl_unset_tip = (!sl_present).then(|| t!("toolbar.sl_unset_tip").to_string());
        // A core-config rejection surfaces as the Size and Sell pods' tooltip: the numbers on the
        // row are local config, but the gear popup's write to this core can still be refused.
        let manual_block_tip = core_config_rejection_caption(
            core_config_edit
                .as_ref()
                .and_then(|row| row.mismatches.as_ref()),
        );
        // A cap is one exchange account's rule. In Overview the readout names the scope as the
        // reason, so value and tooltip come from ONE value and cannot disagree about why it is
        // absent.
        let max_order = if overview {
            MaxOrderReadout::OutOfScope
        } else {
            max_order
        };
        let size_cells = strips::FittedCells::fit(cx, strips::size_labels(size_values));
        let sell_cells = strips::FittedCells::fit(cx, strips::sell_labels(exit.fixed_sell_pcts))
            .with_main(cx, tp_value.clone());
        Self {
            group: group.to_string(),
            follow,
            display_core,
            write_matches_display,
            size_cells,
            size_sel,
            own_on,
            sell_cells,
            sell_slot: exit.fixed_sell_slot,
            manual_on,
            tp_value,
            tp_engaged,
            tp_available,
            tp_tip,
            sl_value,
            sl_present,
            sl_on,
            sl_locked,
            sl_available,
            sl_unset_tip,
            lev_value: lev_value.unwrap_or_else(|| DASH.to_string()),
            lev_present,
            lev_available,
            lev_unset_tip: lev_unset_key.map(|key| t!(key).to_string()),
            lev_of: max_leverage.map(|max| format!("{} ·", t!("toolbar.lev_of", max = max))),
            max_order_inline: t!("toolbar.max_order_inline").to_string(),
            max_order_value: max_order.format_compact(fmt::compact_si, quote),
            max_order_present: max_order.value().is_some(),
            max_order_tip: t!(max_order.tooltip_key()).to_string(),
            no_coin: shows_no_coin_hint(lev_unset_key),
            no_coin_text: t!("toolbar.no_coin_hint").to_string(),
            manual_block_tip,
            lock,
            live_label: if follow {
                t!("toolbar.live").to_string()
            } else {
                t!("toolbar.pause").to_string()
            },
            captions: PodCaptions::read(),
            strat,
        }
    }
}

/// Whether an order write from `group`'s toolbar would reach the source the row displays — the
/// event-time form of [`ToolbarModel::interactive`]: false with no cores, or with the displayed
/// core disagreeing with the write target.
pub(crate) fn order_writes_reach_display(backend: &Entity<Backend>, group: &str, cx: &App) -> bool {
    let display_core = effective_chart_display_core(backend, group, cx);
    let b = backend.read(cx);
    !b.group_cores(group).is_empty() && b.manual_display_matches_write(group, display_core)
}

/// The toolbar states the layout tests draw.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SynthToolbar {
    Normal,
    /// MANUAL with the Strategy pod and its summary; the strategy owns the exits.
    Manual,
    NoCoin,
    LockedOtherCore,
    NoCores,
    SlOff,
    Paused,
}

#[cfg(test)]
impl ToolbarModel {
    /// A synthetic toolbar in `state`, measured with the live locale and fonts.
    pub(crate) fn synthetic(state: SynthToolbar, cx: &App) -> Self {
        use SynthToolbar::*;
        let manual_on = state == Manual;
        let follow = state != Paused;
        let tp_value = "+2.50%".to_string();
        let size_cells = strips::FittedCells::fit(
            cx,
            strips::size_labels([10.0, 25.0, 50.0, 100.0, 250.0, 1000.0]),
        );
        let sell_cells =
            strips::FittedCells::fit(cx, strips::sell_labels([0.5, 1.0, 1.5, 2.0, 3.0, 5.0]))
                .with_main(cx, tp_value.clone());
        let lock = match state {
            NoCores => Some(ToolbarLock::NoCores),
            LockedOtherCore => Some(ToolbarLock::OtherCore("Binance futures main".into())),
            _ => None,
        };
        let has_core = state != NoCores;
        let sl_on = state != SlOff;
        let no_coin = state == NoCoin;
        Self {
            group: "main".into(),
            follow,
            display_core: has_core.then_some(1),
            write_matches_display: lock.is_none(),
            size_cells,
            size_sel: 2,
            own_on: false,
            sell_cells,
            sell_slot: Some(1),
            manual_on,
            tp_value,
            tp_engaged: false,
            tp_available: has_core && !manual_on,
            tp_tip: None,
            sl_value: "-20.00%".into(),
            sl_present: true,
            sl_on,
            sl_locked: manual_on,
            sl_available: has_core,
            sl_unset_tip: None,
            lev_value: if no_coin { DASH.into() } else { "×20".into() },
            lev_present: !no_coin,
            lev_available: has_core && !no_coin,
            lev_unset_tip: None,
            lev_of: (!no_coin).then(|| format!("{} ·", t!("toolbar.lev_of", max = 125))),
            max_order_inline: t!("toolbar.max_order_inline").to_string(),
            max_order_value: "1.25M USDT".into(),
            max_order_present: !no_coin,
            max_order_tip: String::new(),
            no_coin,
            no_coin_text: t!("toolbar.no_coin_hint").to_string(),
            manual_block_tip: None,
            lock,
            live_label: if follow {
                t!("toolbar.live").to_string()
            } else {
                t!("toolbar.pause").to_string()
            },
            captions: PodCaptions::read(),
            strat: manual_on.then(|| StratPod::synthetic(cx)),
        }
    }
}

/// Whether the Strategy pod may be drawn at all: the MANUAL workspace, outside Overview. The pod
/// governs manual orders on ONE core, so neither AUTO nor a scope naming no core draws it; the
/// core must also carry Manual-kind strategies, which [`StratPod::read`] decides.
pub(crate) fn strat_pod_gate(mode: moon_core::config::WorkspaceMode, overview: bool) -> bool {
    mode == moon_core::config::WorkspaceMode::Classic && !overview
}

/// What the toolbar acts through, plus the per-frame popup and editor state `Shell` holds.
pub(crate) struct ToolbarActions {
    pub(crate) backend: Entity<Backend>,
    pub(crate) shell: Entity<Shell>,
    pub(crate) size_edit: Option<(String, usize)>,
    pub(crate) size_input: Entity<MoonInputState>,
    pub(crate) sell_edit: Option<(String, usize)>,
    pub(crate) sell_input: Entity<MoonInputState>,
    /// The open metric and its popup content — ONE fact, so it arrives as one value.
    pub(crate) metric_popup: Option<(TradeMetric, AnyElement)>,
    pub(crate) strat_slot_menu: Option<(CoreId, usize)>,
    pub(crate) strat_slots_open: bool,
    /// The collapsed strip whose caret menu is open, if any.
    pub(crate) caret_open: Option<strips::StripCaret>,
}

/// The toolbar's trailing launchers, in the order they stand on the row and in "More".
fn launch_targets() -> [LaunchTarget; 2] {
    [
        LaunchTarget {
            id: "toolbar-profit-monitor",
            label: t!("toolbar.profit_monitor").to_string(),
            icon: "icons/trending-up.svg",
            labeled_width: None,
            workspace_owner: None,
            open: crate::analytics::profit_monitor::open,
        },
        LaunchTarget {
            id: "toolbar-screener",
            label: t!("toolbar.screener").to_string(),
            icon: "icons/chart-pie.svg",
            labeled_width: None,
            workspace_owner: None,
            open: crate::screener::open,
        },
    ]
}

/// Draw the toolbar row at `level`.
///
/// Every element group the level keeps is drawn, at the width [`toolbar_widths`] counted; the
/// controls the level folds stay reachable through the collapsed forms and "More". With the
/// order controls locked, an overlay sits over the order pods and the strips stay disabled
/// underneath.
///
/// Args:
///     model: The resolved toolbar state.
///     widths: The widths `level` was picked from.
///     level: Collapse level for the current width.
///     available: Toolbar width available, in logical pixels.
///     actions: Backend, shell and per-frame editor and popup state; `None` draws the same
///         geometry with inert controls (layout tests).
///     p: Active palette.
///     cx: Application context used to render.
///
/// Returns:
///     The complete trading toolbar row.
// `use<>` on the return type: the row holds no input lifetime, and saying so is what lets a caller
// build it inside a scope narrower than the tree it is added to — the diagnostic timer around this
// call in `shell::render` is exactly that.
pub(crate) fn render_toolbar(
    mut model: ToolbarModel,
    widths: &ToolbarWidths,
    level: ToolbarLevel,
    available: f32,
    actions: Option<ToolbarActions>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement + use<> {
    let phase_us = crate::diag::timer();
    let mut actions = actions;
    let metric_popup = actions.as_mut().and_then(|a| a.metric_popup.take());
    let (backend, shell, size_edit, sell_edit, strat_slot_menu, strat_slots_open) =
        match actions.as_ref() {
            Some(a) => (
                Some(a.backend.clone()),
                Some(a.shell.clone()),
                a.size_edit.clone(),
                a.sell_edit.clone(),
                a.strat_slot_menu,
                a.strat_slots_open,
            ),
            None => (None, None, None, None, None, false),
        };
    // The strips' editor and caret wiring, built per strip because each owns its own.
    let strip_io = |kind: strips::StripCaret| {
        let a = actions.as_ref()?;
        let shell = a.shell.clone();
        Some(strips::StripIo {
            backend: a.backend.clone(),
            input: match kind {
                strips::StripCaret::Size => &a.size_input,
                strips::StripCaret::Sell => &a.sell_input,
            },
            caret_open: a.caret_open == Some(kind),
            on_caret: std::rc::Rc::new(move |open, _, cx| {
                shell.update(cx, |shell, cx| {
                    shell.set_toolbar_caret(kind, open);
                    cx.notify();
                });
            }),
        })
    };
    let (lev_popup, mut sl_popup, mut tp_popup) = match metric_popup {
        Some((TradeMetric::Lev, content)) => (Some(content), None, None),
        Some((TradeMetric::Sl, content)) => (None, Some(content), None),
        Some((TradeMetric::Tp, content)) => (None, None, Some(content)),
        None => (None, None, None),
    };
    let group = model.group.clone();
    let captions = level.captions();
    // The order controls take edits only where a write reaches the displayed source; the lock
    // overlay states why when they do not. Pods and their "More" rows share these gates.
    let interactive = model.interactive();
    let gates = model.order_gates();
    let tp_open = open_metric_action(TradeMetric::Tp, gates.take, shell.as_ref());
    let sl_open = open_metric_action(TradeMetric::Sl, gates.sl_value, shell.as_ref());
    let sl_write = sl_toggle_action(gates.sl_toggle, &group, backend.as_ref());
    let pod = || design::chrome_pod(cx, p);
    let label = |word: &str, unit: Option<&'static str>| {
        captions.then(|| pod_label(word.to_string(), unit, p, cx))
    };
    crate::diag::record_us(&crate::diag::TOOLBAR_FIT_US, phase_us);
    let phase_us = crate::diag::timer();

    let mut row = h_flex()
        .id("toolbar")
        // Anchors a pinned trailing cluster.
        .relative()
        .w_full()
        .h(px(design::toolbar_height(cx)))
        .flex_none()
        .items_center()
        .gap(design::ui_px(cx, design::CHROME_GAP))
        .px(design::ui_px(cx, design::TOOLBAR_PAD_X))
        .bg(rgb(p.shell_high))
        // The bottom border bounds the whole chrome block (header + toolbar share one background)
        // against the dock region below — it is not a seam between the two rows.
        .border_b_1()
        .border_color(rgb(p.border));
    // The order pods, Strategy through Sell, in one span: the lock overlay covers exactly this
    // span, so Live, the chart tools and "More" stay clickable on a locked row.
    let mut pods = h_flex()
        .id("toolbar-pods")
        .debug_selector(|| "toolbar-pods".into())
        .relative()
        .h_full()
        .flex_none()
        .items_center()
        .gap(design::ui_px(cx, design::CHROME_GAP));

    // Strategy — first, because it decides what the order controls after it mean.
    if let Some(strat) = model.strat.take() {
        let budget = strat_budget(widths, level, available);
        pods = pods.child(
            pod()
                .id("toolbar-pod-strategy")
                .debug_selector(|| "toolbar-pod-strategy".into())
                .children(label(&model.captions.strategy, None))
                .child(strat.render(
                    &group,
                    backend.as_ref().zip(shell.as_ref()),
                    strat_slot_menu,
                    strat_slots_open,
                    level.ms_summary(),
                    budget,
                    p,
                    cx,
                )),
        );
    }

    // Size, with the own-set icon at its end until L7 folds it into "More"; L9 folds the whole
    // control into "More".
    let size_form = if level.size_as_dropdown() {
        StripForm::Collapsed
    } else {
        StripForm::Full
    };
    if !level.size_folded() {
        pods = pods.child(
            pod()
                .id("toolbar-pod-size")
                .debug_selector(|| "toolbar-pod-size".into())
                .children(label(&model.captions.size, Some(SIZE_POD_UNIT)))
                .child(size_strip(
                    &model.size_cells,
                    model.size_sel,
                    // Show the editor only when the request belongs to this toolbar's group.
                    size_edit
                        .filter(|(edit_group, _)| *edit_group == group)
                        .map(|(_, i)| i),
                    strip_io(strips::StripCaret::Size),
                    // `None` disables the strip when the displayed core and the write target
                    // disagree (goal A2 FIX-3) — a live control must not mutate a source other
                    // than the one this row just showed.
                    interactive.then(|| group.clone()),
                    SIZE_POD_UNIT,
                    size_form,
                    cx,
                ))
                .when(!level.own_icon_folded(), |pod| {
                    pod.children(own_trade_toggle(
                        model.display_core,
                        model.own_on,
                        backend.as_ref(),
                        cx,
                    ))
                })
                .when_some(model.manual_block_tip.clone(), |pod, tip| {
                    pod.tooltip(text_tooltip(tip))
                }),
        );
    }

    // Leverage: a leverage edit goes TO THE EXCHANGE, behind an explicit Apply in its popup — a
    // different blast radius from a local preset, so its own pod. With no coin on the chart one
    // hint stands for both it and the order limit.
    if model.no_coin {
        pods = pods.child(
            div()
                .id("toolbar-no-coin")
                .debug_selector(|| "toolbar-no-coin".into())
                .flex_none()
                .max_w(px(no_coin_hint_width(cx, &model.no_coin_text, level)))
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .font_family(design::ui_font())
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(model.no_coin_text.clone())
                .tooltip(text_tooltip(SharedString::from(model.no_coin_text.clone()))),
        );
    } else {
        let max_readout = level.max_readout().then(|| {
            h_flex()
                .id("toolbar-max-order")
                .debug_selector(|| "toolbar-max-order".into())
                .flex_none()
                .items_center()
                .gap(design::ui_px(cx, design::POD_GAP))
                .children(
                    model
                        .lev_of
                        .clone()
                        .map(|text| design::pod_caption(text, p, cx)),
                )
                .child(design::pod_caption(model.max_order_inline.clone(), p, cx))
                .child(strip_text(
                    model.max_order_value.clone(),
                    design::readout_color(p, model.max_order_present),
                    cx,
                ))
                .tooltip(text_tooltip(SharedString::from(
                    model.max_order_tip.clone(),
                )))
        });
        pods = pods.child(
            pod()
                .id("toolbar-pod-lev")
                .debug_selector(|| "toolbar-pod-lev".into())
                .children(label(&model.captions.lev, None))
                .child(metric_button(
                    TradeMetric::Lev,
                    model.lev_value.clone(),
                    design::readout_color(p, model.lev_present),
                    false,
                    lev_popup.is_some(),
                    false,
                    model.lev_available,
                    model.lev_unset_tip.clone().map(SharedString::from),
                    lev_popup,
                    open_metric_action(TradeMetric::Lev, model.lev_available, shell.as_ref()),
                    p,
                    cx,
                ))
                .children(max_readout),
        );
    }

    // Stop: the mini switch (`panic_if_price_drop`) and the value; off strikes the value through.
    let sl_color = if model.sl_present {
        design::danger_color(p)
    } else {
        design::readout_color(p, false)
    };
    let sl_pod = (!level.sl_folded()).then(|| {
        pod()
            .id("toolbar-pod-sl")
            .debug_selector(|| "toolbar-pod-sl".into())
            .children(label(&model.captions.sl, None))
            // Disabled only where the stop is not this terminal's to move: with a manual strategy
            // selected and Moonbot's own rule in force, this reports the strategy's `UseStopLoss`.
            .child(sl_toggle(
                model.sl_on,
                !gates.sl_toggle,
                sl_write.clone(),
                design::theme_installs_roles(cx),
            ))
            .child(metric_button(
                TradeMetric::Sl,
                model.sl_value.clone(),
                sl_color,
                !model.sl_on,
                sl_popup.is_some(),
                false,
                gates.sl_value,
                model.sl_unset_tip.clone().map(SharedString::from),
                sl_popup.take(),
                sl_open.clone(),
                p,
                cx,
            ))
    });
    pods = pods.children(sl_pod);

    // Take and Sell, until L8 folds both into "More". Take: TP and the S presets are one sell
    // target, exactly one of them lit. The value reads as the gain it is, in the positive tone.
    let tp_lit = model.tp_engaged && !model.manual_on;
    let sell_slot = model.sell_slot_shown();
    // Sell: disabled where a click would reach nothing — a manual strategy owning the sell price,
    // or a displayed core disagreeing with the write target (goal A2 FIX-3).
    let sell_group = (!model.manual_on && interactive).then(|| group.clone());
    if !level.take_sell_folded() {
        pods = pods.child(
            pod()
                .id("toolbar-pod-tp")
                .debug_selector(|| "toolbar-pod-tp".into())
                .children(label(&model.captions.tp, None))
                .child(metric_button(
                    TradeMetric::Tp,
                    model.tp_value.clone(),
                    design::positive_color(p),
                    false,
                    tp_popup.is_some(),
                    tp_lit,
                    gates.take,
                    model.tp_tip.clone().map(SharedString::from),
                    tp_popup.take(),
                    tp_open.clone(),
                    p,
                    cx,
                )),
        );
        let sell_form = if level.sell_as_dropdown() {
            StripForm::Collapsed
        } else {
            StripForm::Full
        };
        pods = pods.child(
            pod()
                .id("toolbar-pod-sell")
                .debug_selector(|| "toolbar-pod-sell".into())
                .children(label(&model.captions.sell, None))
                .child(sell_strip(
                    &model.sell_cells,
                    sell_slot,
                    // Show the S editor only when the request belongs to this toolbar's group.
                    sell_edit
                        .filter(|(edit_group, _)| *edit_group == group && !model.manual_on)
                        .map(|(_, i)| i),
                    strip_io(strips::StripCaret::Sell),
                    sell_group.clone(),
                    sell_form,
                    cx,
                ))
                .when_some(model.manual_block_tip.clone(), |pod, tip| {
                    pod.tooltip(text_tooltip(tip))
                }),
        );
    }
    if let Some(lock) = model.lock.as_ref() {
        let room = order_pods_width(widths, level, available, cx);
        pods = pods.child(lock_overlay(lock, room, p, cx));
    }
    crate::diag::record_us(&crate::diag::TOOLBAR_TRADE_US, phase_us);
    let phase_us = crate::diag::timer();

    // Trailing cluster: the chart tools — or "More" once they fold — and the Live chip. When the
    // row still outgrows the window at its narrowest level the cluster pins itself over the row's
    // right edge, because at the end of the flow it would sit past the window's edge.
    let folded_tools = level.chart_tools_folded();
    let mut more_rows: Vec<MoonMenuItem> = Vec::new();
    if level.own_icon_folded() {
        more_rows.extend(own_trade_menu_check(
            model.display_core,
            model.own_on,
            gates.own_set,
            backend.as_ref(),
        ));
    }
    if level.take_sell_folded() {
        more_rows.push(MoonMenuItem::separator());
        more_rows.push(take_menu_row(&model, tp_lit, tp_open.clone()));
        more_rows.extend(sell_menu_rows(
            &model.sell_cells,
            sell_slot,
            backend.clone().zip(sell_group),
        ));
    }
    if level.sl_folded() {
        more_rows.push(MoonMenuItem::separator());
        more_rows.extend(stop_menu_rows(&model, sl_write, sl_open.clone()));
    }
    if level.size_folded() {
        more_rows.push(MoonMenuItem::separator());
        more_rows.extend(size_menu_rows(
            &model.size_cells,
            model.size_sel,
            backend.clone().zip(gates.size.then(|| group.clone())),
        ));
    }
    // A TP or SL popup opened from a "More" row anchors under "More": its pod is not drawn, and
    // the level is never held back to draw it. Present whenever the pod is folded, so the popup's
    // own close path keeps a rendered popover.
    let more_anchors: Vec<AnyElement> = [
        (TradeMetric::Tp, tp_popup, tp_open),
        (TradeMetric::Sl, sl_popup, sl_open),
    ]
    .into_iter()
    .filter(|(metric, ..)| metric_anchors_under_more(*metric, level))
    .map(|(metric, popup, on_open)| metric_popup_anchor(metric, popup, on_open))
    .collect();
    let pinned = trailing_pinned(widths, level, available);
    let backend_live = backend.clone();
    let live = MoonButton::new("toolbar-live")
        .width(live_width(cx, &model.live_label))
        .padding_x(crate::controls::launch::LAUNCHER_PAD_X)
        // Paused is a state the trader must not miss: the chart is frozen off the market.
        .variant(if model.follow {
            MoonButtonVariant::Soft
        } else {
            MoonButtonVariant::Amber
        })
        .tooltip(t!("toolbar.live_tip").to_string())
        .segment(
            MoonButtonSegment::new(LIVE_DOT)
                .color(if model.follow {
                    design::positive_color(p)
                } else {
                    p.amber
                })
                .font_size(9.0)
                .weight(700.0),
        )
        .segment(MoonButtonSegment::new(model.live_label.clone()).weight(500.0))
        .when_some(backend_live, |live, backend| {
            live.on_click(move |_, _, cx| {
                backend.update(cx, |b, bcx| {
                    b.toggle_follow();
                    bcx.notify();
                });
            })
        })
        .render();
    let trailing = design::chrome_section(cx)
        .id("toolbar-trailing")
        .debug_selector(|| "toolbar-trailing".into())
        .when(pinned, |section| {
            section
                .absolute()
                .top_0()
                .bottom_0()
                .right(design::ui_px(cx, design::TOOLBAR_PAD_X))
                .pl(design::ui_px(cx, design::CHROME_GAP))
                .bg(rgb(p.shell_high))
                .border_l_1()
                .border_color(rgb(p.border))
        });
    let trailing = if folded_tools {
        trailing.child(
            div()
                .relative()
                .flex_none()
                .child(overflow_button(
                    "toolbar-overflow",
                    Vec::new(),
                    launch_targets().into(),
                    more_rows,
                    backend.clone(),
                    p,
                    cx,
                ))
                .children(more_anchors),
        )
    } else {
        launch_targets()
            .into_iter()
            .fold(trailing, |section, target| {
                section.child(open_window_button(target, backend.clone(), p, cx))
            })
    }
    .child(
        div()
            .flex_none()
            .debug_selector(|| "toolbar-live".into())
            .child(live),
    );
    row = row.child(pods).child(div().flex_1()).child(trailing);
    crate::diag::record_us(&crate::diag::TOOLBAR_LAUNCH_US, phase_us);
    row
}

/// Whether `metric`'s popup anchors under "More" at `level`: its pod is folded there. Leverage
/// never folds.
pub(crate) fn metric_anchors_under_more(metric: TradeMetric, level: ToolbarLevel) -> bool {
    match metric {
        TradeMetric::Tp => level.take_sell_folded(),
        TradeMetric::Sl => level.sl_folded(),
        TradeMetric::Lev => false,
    }
}

/// The Take pod folded into "More": `Take <value>`, lit like the pod, opening the same TP popup
/// its button opens through the same gated action; the popup then anchors under "More".
fn take_menu_row(model: &ToolbarModel, lit: bool, open: Option<MetricOpen>) -> MoonMenuItem {
    let label = format!("{}  {}", model.captions.tp, model.tp_value);
    let row = match open {
        Some(open) => overflow_row("toolbar-tp-menu", label, move |window, app| {
            open(true, window, app);
        }),
        None => MoonMenuItem::with_key("toolbar-tp-menu", label).disabled(true),
    };
    row.checked(lit)
}

/// The Stop pod folded into "More": its mini switch as a checked row and `Stop <value>` opening
/// the SL popup, each through the pod's own gated action — disabled exactly where the pod's
/// control is. The popup then anchors under "More".
fn stop_menu_rows(
    model: &ToolbarModel,
    write: Option<SlWrite>,
    open: Option<MetricOpen>,
) -> [MoonMenuItem; 2] {
    let on = model.sl_on;
    let toggle_label = model.captions.sl.clone();
    let toggle = match write {
        Some(write) => overflow_row("toolbar-sl-toggle-menu", toggle_label, move |_, app| {
            write(!on, app);
        }),
        None => MoonMenuItem::with_key("toolbar-sl-toggle-menu", toggle_label).disabled(true),
    };
    let value_label = format!("{}  {}", model.captions.sl, model.sl_value);
    let value = match open {
        Some(open) => overflow_row("toolbar-sl-menu", value_label, move |window, app| {
            open(true, window, app);
        }),
        None => MoonMenuItem::with_key("toolbar-sl-menu", value_label).disabled(true),
    };
    [toggle.checked(on), value]
}

/// The sell control folded into "More": one row per S preset, the engaged one checked, each
/// picking through the caret menu's resolver. `act` is `None` where a click would reach nothing.
fn sell_menu_rows(
    cells: &strips::FittedCells,
    sel_slot: Option<usize>,
    act: Option<(Entity<Backend>, String)>,
) -> Vec<MoonMenuItem> {
    (0..6)
        .map(|index| {
            let key = format!("toolbar-sell-more-{index}");
            let label = format!("S{}  {}", index + 1, cells.label(index));
            let row = match act.clone() {
                Some((backend, group)) => overflow_row(key, label, move |_, app| {
                    strips::sell_menu_pick(&backend, &group, index, sel_slot, app);
                }),
                None => MoonMenuItem::with_key(key, label).disabled(true),
            };
            row.checked(sel_slot == Some(index + 1))
        })
        .collect()
}

/// The size control folded into "More": one row per B preset, the selected one checked.
fn size_menu_rows(
    cells: &strips::FittedCells,
    sel: usize,
    act: Option<(Entity<Backend>, String)>,
) -> Vec<MoonMenuItem> {
    (0..6)
        .map(|index| {
            let key = format!("toolbar-size-more-{index}");
            let label = format!("B{}  {}", index + 1, cells.label(index));
            let row = match act.clone() {
                Some((backend, group)) => overflow_row(key, label, move |_, app| {
                    strips::size_menu_pick(&backend, &group, index, app);
                }),
                None => MoonMenuItem::with_key(key, label).disabled(true),
            };
            row.checked(sel == index)
        })
        .collect()
}

/// The lock sentence fitted into `room`: the words before the core name, the name, the words
/// after it — each cut with an ellipsis, in that order, to what the earlier ones leave. Widths are
/// those of the laid-out text boxes.
pub(super) struct LockSentence {
    pub(super) before: (String, f32),
    pub(super) name: Option<(String, f32)>,
    pub(super) after: (String, f32),
    /// The whole uncut sentence, for the tooltip.
    pub(super) full: String,
}

/// Font weight of the core name in the lock sentence.
const LOCK_NAME_WEIGHT: f32 = 500.0;

impl LockSentence {
    /// Split `lock`'s sentence around the core name and fit it into `room`.
    pub(super) fn fit(lock: &ToolbarLock, room: f32, cx: &App) -> Self {
        // The sentence is localized around the name, so it is split at a marker the translation
        // carries through unchanged.
        const NAME_MARK: &str = "\u{1}";
        let (before, name, after) = match lock {
            ToolbarLock::OtherCore(name) => {
                let text = t!("toolbar.locked_other_core", name = NAME_MARK).to_string();
                let (before, after) = text.split_once(NAME_MARK).unwrap_or((text.as_str(), ""));
                (before.to_string(), Some(name.clone()), after.to_string())
            }
            ToolbarLock::NoCores => (
                t!("toolbar.locked_no_cores").to_string(),
                None,
                String::new(),
            ),
        };
        let full = format!("{before}{}{after}", name.as_deref().unwrap_or(""));
        let words = |text: &str| {
            design::ui_text_width_zoomed(cx, text, design::BODY_TEXT, 400.0, false).ceil()
        };
        let mono = |text: &str| design::mono_body_text_width(cx, text, LOCK_NAME_WEIGHT).ceil();
        // A part with no room left even for its ellipsis is dropped; the tooltip still has it.
        let fit = |text: &str, max_w: f32, measure: &dyn Fn(&str) -> f32| {
            let (cut, w) = design::fit_text(text, max_w.max(0.0), measure);
            if text.is_empty() || w > max_w {
                (String::new(), 0.0)
            } else {
                (cut, w)
            }
        };
        let before = fit(&before, room, &words);
        let name = name.map(|name| fit(&name, room - before.1, &mono));
        let used = before.1 + name.as_ref().map_or(0.0, |n| n.1);
        let after = fit(&after, room - used, &words);
        Self {
            before,
            name,
            after,
            full,
        }
    }
}

/// The overlay over the locked order pods: says why they take no edits, the other core's name in
/// mono, fitted into the pods' span with the whole sentence in its tooltip. Occludes the pods so
/// no click reaches a control underneath; Live, the chart tools and "More" stay outside it.
fn lock_overlay(lock: &ToolbarLock, room: f32, p: MoonPalette, cx: &App) -> impl IntoElement {
    let sentence = LockSentence::fit(lock, room, cx);
    let text = |(text, w): (String, f32), id: &'static str| {
        div()
            .flex_none()
            .w(px(w))
            .whitespace_nowrap()
            .debug_selector(move || id.into())
            .font_family(design::ui_font())
            .text_size(design::t_body(cx))
            .text_color(rgb(p.text_soft))
            .child(text)
    };
    h_flex()
        .id("toolbar-lock")
        .debug_selector(|| "toolbar-lock".into())
        .absolute()
        .inset_0()
        .occlude()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .bg(design::moon_alpha(p.panel_high, LOCK_ALPHA))
        .tooltip(text_tooltip(SharedString::from(sentence.full)))
        .child(text(sentence.before, "toolbar-lock-before"))
        .children(sentence.name.map(|(name, w)| {
            div()
                .flex_none()
                .w(px(w))
                .whitespace_nowrap()
                .debug_selector(|| "toolbar-lock-name".into())
                .font_family(design::mono())
                .font_weight(FontWeight(LOCK_NAME_WEIGHT))
                .text_size(design::t_body(cx))
                .text_color(rgb(p.text))
                .child(name)
        }))
        .child(text(sentence.after, "toolbar-lock-after"))
}

/// Opacity of the lock overlay's veil over the order pods.
const LOCK_ALPHA: f32 = 0.85;
