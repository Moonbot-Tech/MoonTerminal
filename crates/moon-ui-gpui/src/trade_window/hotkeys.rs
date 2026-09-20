//! Window-local hotkey routing for the trade-detail window.
//!
//! This window filters instead of calling the shared `apply` / `pre_dispatch` helpers: its chart
//! is frozen, it belongs to no window group and has no trading target. `pre_dispatch` would be
//! actively dangerous here — every `ChartPanel`, the historical one included, registers itself as
//! `Backend::hovered_chart` (`panels/chart/render_input.rs:982`), so a `FigUndo` would delete the
//! user's last figure on the LIVE market, locally, with nothing to bring it back, and would
//! additionally send the removal to the core when that figure carries a chart alert
//! (`backend/figures.rs:345-361`: `chart_alert_delete` fires only under `if fig.alert`).
//!
//! This window contains no text input of any kind, so the `typing` flag both handlers compute is
//! always `false` here today. It is passed anyway because the resolver's signature asks for the
//! answer rather than letting each window invent the policy (`crate::hotkeys::resolve`'s own doc
//! says so), and because a text field added to the ⚙ popup later must be honoured without anyone
//! remembering to come back.
//!
//! Skipping `crate::hotkeys::pre_dispatch` also skips its auto-repeat suppression, and that costs
//! nothing ONLY because none of the four routed actions appears in
//! `HotkeyAction::suppress_on_repeat` (`crate::hotkeys`, the `suppress_on_repeat` match) — a held
//! Scale + steps repeatedly in every window, which is the intended behaviour. If one of the four
//! is later added to `suppress_on_repeat`, repeats would be suppressed in the other two windows
//! and silently NOT suppressed here.
//!
//! A press while the ⚙ settings popup is open is DELIBERATELY not guarded: the popup is mouse-only,
//! holds no keyboard focus, and a window-level zoom has nothing to fight it over, so the chart
//! under it steps as it would with the popup closed. Escape is the opposite case and keeps its
//! existing `settings_open` branch in `on_key`, which closes the popup first.

use gpui::{Context, KeyDownEvent, ModifiersChangedEvent, Window};

use super::TradeWindowView;

/// What a resolved binding means in THIS window: the only two things a frozen chart can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TradeHotkey {
    /// Step the price scale one preset; `up` is Moonbot's "+" — a WIDER band, zooming OUT.
    ScaleStep { up: bool },
    /// Step the time axis with the three-second floor.
    SuperZoom { zoom_in: bool },
}

/// Pure routing table, exhaustive over [`crate::hotkeys::HotkeyAction`] (no wildcard arm), so a
/// new variant forces a decision here. Every non-handled arm returns `None`.
pub(super) fn route(action: crate::hotkeys::HotkeyAction) -> Option<TradeHotkey> {
    use crate::hotkeys::HotkeyAction as A;
    match action {
        A::ScalePlus => Some(TradeHotkey::ScaleStep { up: true }),
        A::ScaleMinus => Some(TradeHotkey::ScaleStep { up: false }),
        A::SuperZoomIn => Some(TradeHotkey::SuperZoom { zoom_in: true }),
        A::SuperZoomOut => Some(TradeHotkey::SuperZoom { zoom_in: false }),
        A::OrderSize(_)
        | A::SellPreset(_)
        | A::ManualStrategy(_)
        | A::CancelBuy
        | A::CancelAllBuys
        | A::PanicSell
        | A::PanicSellOne
        | A::JoinSells
        | A::ShiftOrder { .. }
        | A::SellsToRect
        | A::SplitOrder { .. }
        | A::NewLong
        | A::NewShort
        | A::FigTool(_)
        | A::SwitchFigure
        | A::SwitchCharts
        | A::FigDelete
        | A::FigUndo
        | A::FigAlert
        | A::CloseActiveChart
        | A::ResetWindows
        | A::CancelHoveredOrder
        | A::CloseAllCharts
        | A::CenterChart
        | A::ChartShot => None,
    }
}

impl TradeWindowView {
    /// Handle a trade-window hotkey through the shared recognizer, then this window's filter.
    ///
    /// Args:
    ///     ev: Key-down event to resolve against the configured hotkeys.
    ///     window: Trade window receiving the event.
    ///     cx: View context used to dispatch the resolved action.
    pub(super) fn on_hotkey(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let typing = window.is_text_input_active();
        let action = {
            let b = self.backend.read(cx);
            crate::hotkeys::resolve(ev, &b.preview.as_ref().unwrap_or(&b.config).hotkeys, typing)
        };
        let Some(key) = action.and_then(route) else {
            return;
        };
        if self.dispatch_hotkey(key, cx) {
            cx.stop_propagation();
        }
    }

    /// Route a hotkey bound to Caps Lock or to a lone modifier, which arrive as a modifier change.
    ///
    /// Args:
    ///     ev: Modifier-change event to resolve against the configured hotkeys.
    ///     window: Trade window receiving the event.
    ///     cx: View context used to dispatch the resolved action.
    pub(super) fn on_modifier_hotkey(
        &mut self,
        ev: &ModifiersChangedEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let typing = window.is_text_input_active();
        let action = {
            let b = self.backend.read(cx);
            let hk = &b.preview.as_ref().unwrap_or(&b.config).hotkeys;
            crate::hotkeys::resolve_modifiers(&mut self.modifier_watch, ev, hk, typing)
        };
        let Some(key) = action.and_then(route) else {
            return;
        };
        if self.dispatch_hotkey(key, cx) {
            cx.stop_propagation();
        }
    }

    /// Execute one window-local action against this frozen chart.
    ///
    /// Args:
    ///     key: The action this window's filter accepted.
    ///     cx: View context used to step the scale or the time axis.
    ///
    /// Returns:
    ///     Whether the action was handled here, which is what decides propagation.
    fn dispatch_hotkey(&mut self, key: TradeHotkey, cx: &mut Context<Self>) -> bool {
        match key {
            TradeHotkey::ScaleStep { up } => {
                let next = crate::controls::step_scale(self.scale(cx), up);
                self.pick_scale(next, cx);
                true
            }
            TradeHotkey::SuperZoom { zoom_in } => {
                self.panel
                    .update(cx, |panel, pcx| panel.super_zoom(zoom_in, pcx));
                true
            }
        }
    }
}

#[cfg(test)]
mod tests;
