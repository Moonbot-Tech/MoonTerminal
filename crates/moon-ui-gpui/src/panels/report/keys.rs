//! Report-local shortcuts and their menu hints, resolved from one keymap.

use std::collections::HashSet;

use gpui::{App, Context, KeyDownEvent, Keystroke, Window};
use gpui_component::{input::Copy, kbd::Kbd};
use moon_core::config::HotkeysConfig;

use super::ReportPanel;
use crate::hotkeys::{DISPATCH, Step, binding_id};

/// The row commands available from the focused Report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ReportKey {
    OpenWindow,
    ShowOnChart,
    TradeLog,
    FilterCoin,
    CopyTicker,
}

/// The selected strokes shared by key dispatch and muted menu hints.
pub(super) struct ReportKeymap(Vec<(ReportKey, Keystroke)>);

/// Choose the first candidate each configured window hotkey leaves available.
pub(super) fn report_keymap(hotkeys: &HotkeysConfig) -> ReportKeymap {
    use ReportKey::*;

    let copy = if cfg!(target_os = "macos") {
        "cmd-c"
    } else {
        "ctrl-c"
    };
    let candidates: [(ReportKey, &[&str]); 5] = [
        (OpenWindow, &["enter", "shift-enter", "ctrl-enter"]),
        (ShowOnChart, &["g", "shift-g", "ctrl-shift-g"]),
        (TradeLog, &["l", "shift-l"]),
        (FilterCoin, &["f", "shift-f"]),
        (CopyTicker, &[copy]),
    ];
    // These are configured strokes, so compare their canonical identities before dispatch.
    let mut occupied = HashSet::new();
    for step in DISPATCH {
        match step {
            Step::Slot(slot) => occupied.extend(binding_id(hotkeys.key(*slot))),
            Step::Builtin(builtin) => {
                occupied.extend(builtin.strokes.iter().filter_map(|raw| binding_id(raw)))
            }
        }
    }
    ReportKeymap(
        candidates
            .into_iter()
            .filter_map(|(action, candidates)| {
                candidates
                    .iter()
                    .filter_map(|candidate| crate::hotkeys::parse_binding(candidate))
                    .find(|stroke| !occupied.contains(&(stroke.modifiers, stroke.key.clone())))
                    .map(|stroke| (action, stroke))
            })
            .collect(),
    )
}

impl ReportKeymap {
    /// Match the physical letter and modifiers, ignoring the event's typed character.
    pub(super) fn matches(&self, keystroke: &Keystroke) -> Option<ReportKey> {
        let event = crate::hotkeys::recorded_keystroke(keystroke.clone());
        self.0.iter().find_map(|(action, stroke)| {
            (stroke.key == event.key && stroke.modifiers == event.modifiers).then_some(*action)
        })
    }

    /// Format exactly the stroke that dispatch will recognize, using platform key names.
    pub(super) fn hint(&self, action: ReportKey) -> Option<String> {
        self.0
            .iter()
            .find(|(candidate, _)| *candidate == action)
            .map(|(_, stroke)| Kbd::format(stroke))
    }
}

/// Reject strokes outside the raw Report candidates before resolving any configured bindings.
fn is_row_key_candidate(stroke: &Keystroke) -> bool {
    let m = stroke.modifiers;
    if m.alt || m.platform || m.function {
        return false;
    }
    let event = crate::hotkeys::recorded_keystroke(stroke.clone());
    match event.key.as_str() {
        "enter" => !(m.control && m.shift),
        "g" => !m.control || m.shift,
        "l" | "f" => !m.control,
        _ => false,
    }
}

/// Keep chart and log shortcuts available to other handlers on a multi-selection.
pub(super) fn for_selection(action: ReportKey, rows: usize) -> Option<ReportKey> {
    match (action, rows) {
        (_, 0) | (ReportKey::ShowOnChart | ReportKey::TradeLog, 2..) => None,
        _ => Some(action),
    }
}

impl ReportPanel {
    /// Read the live settings draft when present, as the shell's window hotkeys do.
    pub(super) fn report_keys(&self, cx: &App) -> ReportKeymap {
        let backend = self.backend.read(cx);
        report_keymap(&backend.preview.as_ref().unwrap_or(&backend.config).hotkeys)
    }

    /// Leave keys with the focused editor or popup while it accepts input.
    fn row_shortcuts_active(&self, window: &Window, cx: &App) -> bool {
        let root = super::Root::read(window, cx);
        !window.is_text_input_active()
            && !root.has_active_context_menu()
            && root.active_dialog_count() == 0
            && !self.coin_popup_open
            && !self.filter_focus.contains_focused(window, cx)
    }

    /// Dispatch the raw row keys once per press. Copy arrives through the root's bound action.
    pub(super) fn on_report_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !event.is_held
            && is_row_key_candidate(&event.keystroke)
            && self.row_shortcuts_active(window, cx)
            && let Some(action) = self.report_keys(cx).matches(&event.keystroke)
            && action != ReportKey::CopyTicker
            && self.run_report_key(action, window, cx)
        {
            cx.stop_propagation();
        }
    }

    /// Handle the same Copy action bound by MoonUI's root so this focus path takes precedence.
    pub(super) fn on_report_copy(&mut self, _: &Copy, window: &mut Window, cx: &mut Context<Self>) {
        // GPUI stops propagation by default at each bubble-phase action listener.
        if self.row_shortcuts_active(window, cx)
            && self.report_keys(cx).hint(ReportKey::CopyTicker).is_some()
            && self.run_report_key(ReportKey::CopyTicker, window, cx)
        {
            cx.stop_propagation();
        } else {
            cx.propagate();
        }
    }
}

#[cfg(test)]
mod tests;
