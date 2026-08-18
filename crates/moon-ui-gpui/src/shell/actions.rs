//! Drains backend requests for inline order-size and fixed-sell editing, presents Engine-action
//! results, and routes group-window root hotkeys. Extracted from `shell/mod.rs` without changing
//! behavior.

use gpui::*;

use super::Shell;
use crate::controls;

/// Take an inline-edit request only when it belongs to the observing Shell's group.
fn take_group_edit(
    request: &mut Option<(String, usize)>,
    shell_group: &str,
) -> Option<(String, usize)> {
    request
        .as_ref()
        .is_some_and(|(request_group, _)| request_group == shell_group)
        .then(|| request.take())
        .flatten()
}

/// Prefer a hovered chart target only when it belongs to the hotkey receiver's window group.
fn select_hotkey_target(
    hovered: Option<(moon_core::session::CoreId, String)>,
    hovered_belongs_to_group: bool,
    main: Option<(moon_core::session::CoreId, String)>,
) -> Option<(moon_core::session::CoreId, String)> {
    hovered.filter(|_| hovered_belongs_to_group).or(main)
}

impl Shell {
    /// Open and focus this group's pending F1-F6 inline editor without consuming another group's.
    pub(super) fn drain_order_size_edit_request(&mut self, cx: &mut Context<Self>) {
        let group = self.group.clone();
        let edit_req = self.backend.update(cx, |b, _| {
            take_group_edit(&mut b.order_size_edit_req, &group)
        });
        let Some((group, ix)) = edit_req.filter(|(_, i)| *i < 6) else {
            return;
        };
        let cur = self.backend.read(cx).order_size_value(&group, ix);
        self.size_edit = Some((group, ix));
        let input = self.size_input.clone();
        let value = controls::fmt_adaptive(cur);
        let handle = self.window_handle;
        cx.defer(move |app| {
            let _ = handle.update(app, move |_, window, app| {
                input.update(app, |st, cx| {
                    st.set_value(value, window, cx);
                    st.focus(window, cx);
                });
            });
        });
    }

    /// Drain Engine-action results such as leverage, hedge, cancel-all, and transfers into toasts.
    ///
    /// The queue is shared in `CoreStore`, so only the active window's Shell drains it; otherwise
    /// every group window observing the same Backend would show the same toast. While no window is
    /// active, results accumulate up to the `CoreData` cap and appear after activation and notify.
    pub(super) fn drain_engine_action_toasts(&mut self, cx: &mut Context<Self>) {
        if !self.window_active {
            return;
        }
        let toasts = self
            .backend
            .update(cx, |b, _| b.session.take_engine_action_toasts());
        if toasts.is_empty() {
            return;
        }
        let handle = self.window_handle;
        cx.defer(move |app| {
            let _ = handle.update(app, move |_, window, app| {
                use moon_ui::MoonWindowExt as _;
                for (core, r) in toasts {
                    let action = engine_action_label(&r.kind);
                    let note = if r.success {
                        moon_ui::MoonNotification::success(action)
                            .title(rust_i18n::t!("shell.engine_toast.ok", core = core).to_string())
                    } else {
                        let mut err = r.error_msg.clone();
                        if err.is_empty() {
                            err = format!("code {}", r.error_code);
                        }
                        moon_ui::MoonNotification::error(format!("{action}: {err}"))
                            .title(
                                rust_i18n::t!("shell.engine_toast.fail", core = core).to_string(),
                            )
                            .autohide(false)
                    };
                    window.push_notification(note, app);
                }
            });
        });
    }

    /// Open and focus the inline editor after a fixed-sell S button is double-clicked.
    /// Seeds it with the group's current preset percentage, mirroring the order-size editor.
    pub(super) fn drain_sell_edit_request(&mut self, cx: &mut Context<Self>) {
        let group = self.group.clone();
        let edit_req = self
            .backend
            .update(cx, |b, _| take_group_edit(&mut b.sell_edit_req, &group));
        let Some((group, ix)) = edit_req.filter(|(_, i)| *i < 6) else {
            return;
        };
        let cur = self.backend.read(cx).fixed_sell_pct(&group, ix);
        self.sell_edit = Some((group, ix));
        let input = self.sell_input.clone();
        let value = controls::fmt_adaptive(cur);
        let handle = self.window_handle;
        cx.defer(move |app| {
            let _ = handle.update(app, move |_, window, app| {
                input.update(app, |st, cx| {
                    st.set_value(value, window, cx);
                    st.focus(window, cx);
                });
            });
        });
    }

    /// Route a group-window root hotkey through the shared [`crate::hotkeys`] resolver.
    ///
    /// Actions delegated to [`crate::hotkeys::apply`] target the group's fullscreen Main-chart
    /// market and active trading core. NewLong and NewShort are exceptions routed through the
    /// globally hovered chart. Scaling uses the group's revision mechanism rather than `apply`.
    /// Stop propagation only when the resolved action reports that it was handled.
    ///
    /// Args:
    ///     ev: Key-down event to resolve against built-in and configured shortcuts.
    ///     window: The window the key arrived at. The chart shot needs it to reach the OS window
    ///         behind the chart and to report what it managed to copy.
    ///     cx: Shell context used to route the resolved action.
    ///
    /// Returns:
    ///     Nothing; handled actions stop event propagation.
    pub(super) fn on_hotkey(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::hotkeys::HotkeyAction;
        // Before the bindings are even consulted: Escape leaves the Sells-to-zone mode, whatever
        // modifier the drawing hand still holds down.
        if crate::hotkeys::escape_leaves_sells_zone(ev, &self.backend, cx) {
            cx.stop_propagation();
            return;
        }
        let action = {
            let b = self.backend.read(cx);
            let hk = &b.preview.as_ref().unwrap_or(&b.config).hotkeys;
            crate::hotkeys::resolve(ev, hk)
        };
        let Some(action) = action else {
            return;
        };
        // Action-owned policies run before this window's routing: auto-repeat suppression, and the
        // cursor-addressed target that makes Split act on the order under the pointer.
        if crate::hotkeys::pre_dispatch(action, ev, &self.backend, cx) {
            cx.stop_propagation();
            return;
        }
        let group = self.group.clone();
        let handled = match action {
            // Step the active tab's Y scale and address the revision to this group. Its ChartTabs
            // applies the change to the active panel and consumes only revisions for its group.
            HotkeyAction::ScalePlus | HotkeyAction::ScaleMinus => {
                let zoom_in = matches!(action, HotkeyAction::ScalePlus);
                self.backend.update(cx, |b, _| {
                    let next = crate::controls::step_scale(b.price_scale, zoom_in);
                    b.price_scale = next;
                    b.price_scale_group = Some(group.clone());
                    b.price_scale_rev = b.price_scale_rev.wrapping_add(1);
                });
                true
            }
            // Advance this group's active fullscreen Main chart through a dedicated group revision.
            // Keeping it separate from the scale revision prevents switching and zooming from
            // consuming each other's signals.
            HotkeyAction::SwitchCharts => {
                self.backend.update(cx, |b, _| {
                    b.switch_charts_group = Some(group.clone());
                    b.switch_charts_rev = b.switch_charts_rev.wrapping_add(1);
                });
                true
            }
            // Place a manual order through the globally hovered chart, which alone can translate
            // the cursor position into a price. This is independent of which window has focus.
            HotkeyAction::NewLong | HotkeyAction::NewShort => {
                let short = matches!(action, HotkeyAction::NewShort);
                let chart = self
                    .backend
                    .read(cx)
                    .hovered_chart
                    .clone()
                    .and_then(|w| w.upgrade());
                match chart {
                    Some(chart) => chart.update(cx, |p, pcx| p.place_order_at_cursor(short, pcx)),
                    None => false,
                }
            }
            // The chart shot is scoped to THIS window: a group window has no single unambiguous
            // chart, so it resolves through the hover trail recorded for its own window rather
            // than naming a panel here.
            HotkeyAction::ChartShot => {
                crate::panels::shot::copy_active_chart(&self.backend, window, cx)
            }
            // Built-in Ctrl+Shift+F10 restores all windows to on-screen positions.
            HotkeyAction::ResetWindows => {
                crate::window::windowing::reset_all_windows_onscreen(cx);
                true
            }
            // Built-in Tab/Delete cancels the order tracked by the hovered chart.
            HotkeyAction::CancelHoveredOrder => {
                crate::hotkeys::cancel_hovered_order(&self.backend, cx)
            }
            // Delete prioritizes the selected figure, then falls back to hovered-order cancellation.
            // The default figure binding resolves before built-in Tab/Delete and would otherwise
            // shadow order cancellation permanently.
            HotkeyAction::FigDelete => {
                self.backend.update(cx, |b, bcx| {
                    // Figure deletion does not use a chart target or active core.
                    crate::hotkeys::apply(action, b, bcx, &group, None, None)
                }) || crate::hotkeys::cancel_hovered_order(&self.backend, cx)
            }
            // Built-in Shift+Escape increments the global revision that closes every Main stack.
            HotkeyAction::CloseAllCharts => {
                self.backend.update(cx, |b, _| {
                    b.close_all_charts_rev = b.close_all_charts_rev.wrapping_add(1);
                });
                true
            }
            // Built-in Escape closes this group's active Main chart in either presentation. As in
            // Moonbot, it leaves drawing mode enabled and does not close the group or detached window.
            HotkeyAction::CloseActiveChart => {
                self.backend.update(cx, |b, bcx| {
                    b.close_active_chart_group = Some(group.clone());
                    b.close_active_chart_rev = b.close_active_chart_rev.wrapping_add(1);
                    // ChartTabs consumes this revision through its Backend observer. Without an
                    // explicit notification, Escape waits for an unrelated market-data repaint.
                    bcx.notify();
                });
                true
            }
            other => {
                let hovered_target = self
                    .backend
                    .read(cx)
                    .hovered_chart
                    .clone()
                    .and_then(|weak| weak.upgrade())
                    .and_then(|chart| chart.read(cx).target_at_cursor());
                self.backend.update(cx, |b, bcx| {
                    let hovered_belongs_to_group = hovered_target
                        .as_ref()
                        .is_some_and(|(core, _)| b.core_belongs_to_group(&group, *core));
                    let target = select_hotkey_target(
                        hovered_target,
                        hovered_belongs_to_group,
                        b.main_chart_target(&group),
                    );
                    let active_core = b.active_trade_core(&group);
                    crate::hotkeys::apply(other, b, bcx, &group, target, active_core)
                })
            }
        };
        if handled {
            cx.stop_propagation();
        }
    }
}

/// Build the human-readable Engine-action label for a result accepted or rejected by the core.
/// Wallet names use `WalletKind::label()`, matching the Assets tree.
fn engine_action_label(kind: &moon_core::feed::EngineActionKind) -> String {
    use moon_core::feed::EngineActionKind as K;
    use rust_i18n::t;
    match kind {
        K::CancelAllOrders => t!("shell.engine_action.cancel_all").to_string(),
        K::SetLeverage { market, leverage } => t!(
            "shell.engine_action.set_leverage",
            market = market,
            lev = leverage
        )
        .to_string(),
        K::SetHedgeMode { on: true } => t!("shell.engine_action.hedge_on").to_string(),
        K::SetHedgeMode { on: false } => t!("shell.engine_action.hedge_off").to_string(),
        K::ChangePositionType { market } => {
            t!("shell.engine_action.change_position_type", market = market).to_string()
        }
        K::ConvertDust => t!("shell.engine_action.convert_dust").to_string(),
        K::ConfirmRiskLimit { market } => {
            t!("shell.engine_action.confirm_risk_limit", market = market).to_string()
        }
        K::SetMaMode { on: true } => t!("shell.engine_action.ma_on").to_string(),
        K::SetMaMode { on: false } => t!("shell.engine_action.ma_off").to_string(),
        K::TransferAsset {
            asset,
            qty,
            from,
            to,
        } => t!(
            "shell.engine_action.transfer",
            qty = controls::fmt_adaptive(*qty),
            asset = asset,
            from = from.label(),
            to = to.label()
        )
        .to_string(),
        K::ReloadOrderBook => t!("shell.engine_action.reload_order_book").to_string(),
    }
}

#[cfg(test)]
mod tests;
