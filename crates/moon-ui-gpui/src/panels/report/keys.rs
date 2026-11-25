//! Report-local shortcuts and their menu hints, resolved from one keymap.

use std::collections::HashSet;

use gpui::{App, Context, KeyDownEvent, Keystroke, Window};
use gpui_component::{input::Copy, kbd::Kbd};
use moon_core::config::HotkeysConfig;

use super::ReportPanel;
use crate::hotkeys::{BindingId, DISPATCH, Step, parse_binding, recorded_keystroke};

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
    // The shell accepts native-layout letters as well as their physical Latin equivalents.
    let mut occupied = HashSet::new();
    for step in DISPATCH {
        match step {
            Step::Slot(slot) => occupied.extend(parse_binding(hotkeys.key(*slot)).map(physical_id)),
            Step::Builtin(builtin) => occupied.extend(
                builtin
                    .strokes
                    .iter()
                    .filter_map(|raw| parse_binding(raw))
                    .map(physical_id),
            ),
        }
    }
    ReportKeymap(
        candidates
            .into_iter()
            .filter_map(|(action, candidates)| {
                candidates
                    .iter()
                    .filter_map(|candidate| parse_binding(candidate))
                    .find(|stroke| !occupied.contains(&physical_id(stroke.clone())))
                    .map(|stroke| (action, stroke))
            })
            .collect(),
    )
}

/// Compare bindings and events by the same physical letter and modifiers.
fn physical_id(stroke: Keystroke) -> BindingId {
    let stroke = recorded_keystroke(stroke);
    (stroke.modifiers, stroke.key)
}

impl ReportKeymap {
    /// Match the physical letter and modifiers, ignoring the event's typed character.
    pub(super) fn matches(&self, keystroke: &Keystroke) -> Option<ReportKey> {
        let event = physical_id(keystroke.clone());
        self.0
            .iter()
            .find_map(|(action, stroke)| (physical_id(stroke.clone()) == event).then_some(*action))
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
    let (m, key) = physical_id(stroke.clone());
    if key == "c" {
        return !m.alt
            && !m.shift
            && !m.function
            && if cfg!(target_os = "macos") {
                m.platform && !m.control
            } else {
                m.control && !m.platform
            };
    }
    if m.alt || m.platform || m.function {
        return false;
    }
    match key.as_str() {
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

    /// Dispatch raw row keys, including native-layout copy when the root's binding does not match.
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
