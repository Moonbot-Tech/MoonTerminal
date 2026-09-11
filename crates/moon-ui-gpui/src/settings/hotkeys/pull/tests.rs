use moon_core::config::MANUAL_STRATEGY_KEYS;
use moon_core::config::moonbot_import::shortcut::{MOD_CTRL, MOD_SHIFT};
use moon_core::feed::{CoreHotkeyAction, CoreHotkeyLayout};

use crate::hotkeys::same_binding;

use super::*;

/// `TShortCut` bits for Ctrl+Shift+Z — what the core sends for the press the terminal spells
/// `ctrl-shift-z`. Through `shortcut`'s own public constants: `decode` reads those same ones, so
/// open-coding the bits would not have pinned the layout any harder than this does.
const CTRL_SHIFT_Z: u16 = MOD_CTRL | MOD_SHIFT | b'Z' as u16;

/// The row the preview built for `Cancel Buy`, given the core's raw key for it.
///
/// One slot is enough for every case here — what differs between them is the terminal's OWN file,
/// which is the side under test.
fn cancel_buy_row(hotkeys: &HotkeysConfig, raw: u16) -> PullRow {
    let layout = CoreHotkeyLayout {
        order_size: [0; 6],
        sell_preset: [0; 6],
        named: CoreHotkeyAction::ALL.map(|a| {
            (
                a,
                if a == CoreHotkeyAction::CancelBuy {
                    raw
                } else {
                    0
                },
            )
        }),
    };
    preview_core_hotkeys(hotkeys, &layout, &[0u16; MANUAL_STRATEGY_KEYS])
        .into_iter()
        .find(|row| row.slot == KeySlot::CancelBuy)
        .expect("the preview covers every named slot")
}

/// Regression target: the pull decided "already bound elsewhere" by comparing STRINGS, so a key the
/// terminal already holds under another spelling read as free and was applied — double-binding the
/// file, after which the dispatcher answers the first branch and the loser silently stops working.
///
/// `shift-ctrl-Z` is a spelling a hand-edited or pasted `hotkeys.toml` really carries:
/// `Keystroke::parse` takes the modifiers in any order and is case-insensitive, so it is the same
/// press as the `ctrl-shift-z` the core offers here.
#[test]
fn a_differently_spelled_rival_is_a_conflict() {
    let mut hotkeys = HotkeysConfig::default();
    // Another slot holds the press, spelled the other way round.
    hotkeys.manual_strategy[0] = "shift-ctrl-Z".to_string();
    // And the slot being pulled holds something else, so this cannot come back `Unchanged`.
    hotkeys.cancel_buy = "alt-b".to_string();

    let row = cancel_buy_row(&hotkeys, CTRL_SHIFT_Z);

    assert_eq!(
        row.verdict,
        PullVerdict::Conflict,
        "the manual-strategy slot already holds this press"
    );
}

/// The other half of the same comparison: a core key the slot ALREADY holds, spelled differently,
/// is not a change at all. Reported as `WillApply` before, which showed the user a diff row for a
/// press they already had and rewrote their own spelling on apply.
#[test]
fn the_same_press_spelled_differently_is_unchanged() {
    let hotkeys = HotkeysConfig {
        cancel_buy: "shift-ctrl-Z".to_string(),
        ..Default::default()
    };

    let row = cancel_buy_row(&hotkeys, CTRL_SHIFT_Z);

    assert_eq!(row.verdict, PullVerdict::Unchanged);
}

/// And a press nothing holds still applies — the guard above must narrow the verdict, not blanket it.
#[test]
fn a_free_press_still_applies() {
    let hotkeys = HotkeysConfig {
        cancel_buy: "alt-b".to_string(),
        ..Default::default()
    };
    // Stated rather than assumed, and compared as PRESSES: a shipped default spelled `Shift-Ctrl-Z`
    // would walk straight past a string comparison and fail the assertion below with a misleading
    // message about the verdict.
    for held in hotkeys.bound_keys() {
        assert!(
            !same_binding(&held, "ctrl-shift-z"),
            "a shipped default holds the press this test needs free: {held}"
        );
    }

    let row = cancel_buy_row(&hotkeys, CTRL_SHIFT_Z);

    assert_eq!(row.verdict, PullVerdict::WillApply);
    assert_eq!(row.new_key.as_deref(), Some("ctrl-shift-z"));
}
