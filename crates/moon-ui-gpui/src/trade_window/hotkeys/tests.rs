// Intended path: crates/moon-ui-gpui/src/trade_window/hotkeys/tests.rs
// Reached from crates/moon-ui-gpui/src/trade_window/hotkeys.rs via `#[cfg(test)] mod tests;`.
//
// Do not use `super::*`: `hotkeys/tests.rs` (the shared hotkeys module) already warns that a
// glob there can shadow the built-in `#[test]`. Name what is needed instead.
use super::{TradeHotkey, route};
use crate::hotkeys::{DISPATCH, HotkeyAction, Step, action_of};
use moon_core::config::HotkeysConfig;

/// Only the four window-local view actions may pass the Trade window's frozen-chart filter, and
/// each must land on the scale/zoom direction its label promises.
///
/// Covers breakage 2 — `trade_window/hotkeys.rs:route` — a wildcard arm forwarding to
/// `crate::hotkeys::apply`/`pre_dispatch` "for parity" would let a key over a frozen REPLAY act
/// on the LIVE market under the pointer in another window (plan finding 0.4), or split an order.
///
/// Covers breakage 4 — same file — `ScalePlus` mapped to `ScaleStep { up: false }` (or the zoom
/// pair swapped), the exact bug `controls/scale.rs:61`'s doc already records happening once.
///
/// Derived from `DISPATCH`, so a newly added slot is exercised automatically rather than falling
/// through the `other` arm untested.
#[test]
fn only_the_window_local_view_actions_route() {
    let hk = HotkeysConfig::default();
    let mut seen = 0u8;
    for step in DISPATCH {
        let action = match step {
            Step::Slot(slot) => action_of(*slot, &hk),
            Step::Builtin(builtin) => builtin.action,
        };
        let routed = route(action);
        let want = match action {
            HotkeyAction::ScalePlus => Some(TradeHotkey::ScaleStep { up: true }),
            HotkeyAction::ScaleMinus => Some(TradeHotkey::ScaleStep { up: false }),
            HotkeyAction::SuperZoomIn => Some(TradeHotkey::SuperZoom { zoom_in: true }),
            HotkeyAction::SuperZoomOut => Some(TradeHotkey::SuperZoom { zoom_in: false }),
            _ => None,
        };
        if want.is_some() {
            seen += 1;
        }
        assert_eq!(
            routed, want,
            "{action:?} must route to exactly {want:?}, not {routed:?}"
        );
    }
    assert_eq!(
        seen, 4,
        "DISPATCH must still carry all four window-local slots for this test to mean anything"
    );
}
