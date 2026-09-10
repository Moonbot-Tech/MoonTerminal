use moon_core::config::{HotkeysConfig, MANUAL_STRATEGY_KEYS};
use moon_core::feed::{CoreHotkeyAction, CoreHotkeyLayout, GestureSettings};

/// A core whose gesture block is entirely unset — enough to enumerate the rows the pull covers,
/// which is all this file asks of it. What each row DOES with a value is `pull_gestures`' own test.
const CORE_ZERO: GestureSettings = GestureSettings {
    buy_set_click: 0,
    short_set_click: 0,
    pending_order_set_click: 0,
    pending_short_set_click: 0,
    same_hotkeys_for_move: false,
    buy_move_click: 0,
    short_buy_move_click: 0,
    replace_buy_kind: 0,
    sell_move_click: 0,
    short_sell_move_click: 0,
    replace_sell_kind: 0,
    buy_move_click_2: 0,
    short_buy_move_click_2: 0,
    replace_buy_kind_2: 0,
    sell_move_click_2: 0,
    short_sell_move_click_2: 0,
    replace_sell_kind_2: 0,
};

use super::super::pull::preview_core_hotkeys;
use super::super::pull_gestures::{GestureTarget, preview_core_gestures};
use super::super::{MouseSlot, MoveKindSlot, all_mouse_slots, all_slots, mouse_slot_id, slot_id};
use super::*;

/// The slot ids a "pull layout from core" would actually reconcile, taken from the pull itself.
///
/// Built through [`preview_core_hotkeys`] rather than through its inner action-to-slot map: the
/// preview is what the button runs, it adds the three preset families the map never sees, and
/// going through it keeps the mapping private.
fn slots_the_pull_writes() -> Vec<String> {
    let layout = CoreHotkeyLayout {
        order_size: [0; 6],
        sell_preset: [0; 6],
        named: CoreHotkeyAction::ALL.map(|action| (action, 0u16)),
    };
    preview_core_hotkeys(
        &HotkeysConfig::default(),
        &layout,
        &[0u16; MANUAL_STRATEGY_KEYS],
    )
    .iter()
    .map(|row| slot_id(row.slot))
    .collect()
}

/// The `Moonbot` mark is a PROMISE — pull a layout or paste a configuration and this row changes —
/// so it is checked against the code that keeps the promise, not against a second hand-written list.
///
/// EVERY slot is checked, in both directions, because a sampled check is what lets a mark drift: a
/// row marked `Moonbot` that nothing writes tells the user their key is at risk when it is not, and
/// a row the pull DOES write while marked `Ours` tells them the opposite — they keep a binding they
/// were told was safe.
///
/// Plausible breakage: a `slot_for_action` arm returning `None` after a core action is dropped, or
/// a new slot wired into the pull and marked `Ours` here.
#[test]
fn the_moonbot_mark_names_exactly_the_slots_the_pull_writes() {
    let written = slots_the_pull_writes();
    assert!(
        written.len() > 20,
        "the pull preview came back nearly empty ({written:?}), so this test proves nothing"
    );
    for slot in all_slots() {
        let id = slot_id(slot);
        assert_eq!(
            key_slot_meta(slot).origin == Origin::Shared,
            written.contains(&id),
            "{id}: the mark and the pull disagree about whether this row is overwritten"
        );
    }
}

/// The gesture marks get the same exhaustive, two-way check the keyboard marks get — against the
/// gesture pull itself rather than a second list.
///
/// The twelve order gestures travel now; the figure gesture has no core counterpart and never will
/// until `GestureSettings` grows one.
///
/// Plausible breakage: a gesture dropped from `preview_core_gestures` while its row still claims a
/// paste will overwrite it, or the reverse.
#[test]
fn the_gesture_marks_name_exactly_the_gestures_the_pull_writes() {
    let written: Vec<GestureTarget> = preview_core_gestures(&HotkeysConfig::default(), &CORE_ZERO)
        .iter()
        .map(|row| row.target)
        .collect();
    for slot in all_mouse_slots() {
        let marked = mouse_slot_meta(slot).origin == Origin::Shared;
        let carried = written.contains(&GestureTarget::Gesture(slot));
        assert_eq!(
            marked,
            carried,
            "{}: the mark and the gesture pull disagree about whether this row is overwritten",
            mouse_slot_id(slot)
        );
    }
    // The two rows that are not slots and so have no `mouse_slot_meta` of their own. Both are
    // written by the pull, so both must carry the mark that says so — the mirror switch especially,
    // since it decides whether the four short rows follow the long ones at all.
    assert!(written.contains(&GestureTarget::SameForMove));
    assert_eq!(SAME_FOR_MOVE.origin, Origin::Shared);
    for slot in [
        MoveKindSlot::BuyMove,
        MoveKindSlot::SellMove,
        MoveKindSlot::BuyMove2,
        MoveKindSlot::SellMove2,
    ] {
        assert!(
            written.contains(&GestureTarget::Kind(slot)),
            "{slot:?} is no longer pulled, but its gesture row still claims Moonbot writes it"
        );
    }
}

/// Pins the surfaces claimed for the rows whose routing is easiest to get wrong.
///
/// Panic Sell is the one worth naming out loud, because the obvious reading of it is wrong: it does
/// NOT simply act on the focused window's chart. `shell/actions.rs::select_hotkey_target` prefers
/// the chart under the POINTER whenever that chart belongs to the same window group, and only falls
/// back to the window's main chart — so the press can hit a market the window is not showing.
///
/// Plausible breakage: `select_hotkey_target` losing its hover branch, or a new trading key added
/// with the window-only mark, either of which makes the page state the wrong market.
#[test]
fn the_aimed_trading_keys_claim_the_pointer_as_well_as_the_window() {
    let aimed = [
        HotkeySlot::PanicSell,
        HotkeySlot::PanicSellOne,
        HotkeySlot::CancelBuy,
        HotkeySlot::JoinSells,
        HotkeySlot::ShiftBuyUp,
        // Both split slots resolve to the same action, so they must not disagree.
        HotkeySlot::SplitOrder,
        HotkeySlot::SplitOrderX,
    ];
    for slot in aimed {
        let scope = key_slot_meta(slot).scope;
        assert!(
            scope.intersects(Scope::CURSOR) && scope.intersects(Scope::WINDOW),
            "{} no longer claims both the pointer and the window",
            slot_id(slot)
        );
    }
    // The group-owned keys are the contrast: no pointer is consulted for them at all.
    for slot in [
        HotkeySlot::OrderSize(0),
        HotkeySlot::SwitchCharts,
        HotkeySlot::ScalePlus,
        HotkeySlot::CancelAllBuys,
    ] {
        assert_eq!(
            key_slot_meta(slot).scope,
            Scope::WINDOW,
            "{}",
            slot_id(slot)
        );
    }
}

/// A key press with nothing selected does not stop at the figure layer — it cancels the order under
/// the pointer — so the row has to admit that second, destructive surface.
#[test]
fn the_figure_delete_key_admits_its_order_cancelling_fallback() {
    let scope = key_slot_meta(HotkeySlot::FigDelete).scope;
    assert!(scope.intersects(Scope::SELECTION));
    assert!(scope.intersects(Scope::CURSOR));
    // The alert toggle has no such fallback: with nothing selected the key is simply unhandled.
    assert_eq!(key_slot_meta(HotkeySlot::FigAlert).scope, Scope::SELECTION);
}

/// A slot whose surface depends on the "separate control zones" setting holds BOTH surfaces, so an
/// overlap test cannot miss the half that is currently switched off.
#[test]
fn a_trading_gesture_holds_both_of_its_possible_surfaces() {
    for scope in [
        mouse_slot_meta(MouseSlot::BuySet).scope,
        mouse_slot_meta(MouseSlot::BuyMove).scope,
        // The manual order keys are placed through the same gate, so they answer the same way.
        key_slot_meta(HotkeySlot::NewLong).scope,
    ] {
        assert!(scope.intersects(Scope::BOOK));
        assert!(scope.intersects(Scope::PLOT));
    }
}

/// Both surfaces reach the label, and the label is real text rather than a missing-key echo.
///
/// The echo matters: rust-i18n answers an unknown key with `"<locale>.<key>"`, which still contains
/// a slash and still contains each part, so a laxer assertion here would pass with every new locale
/// entry deleted.
#[test]
fn a_two_surface_label_names_both_surfaces_in_words() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(Scope::BOOK.label(), "book");
    assert_eq!(Scope::PLOT.label(), "plot");
    assert_eq!(
        mouse_slot_meta(MouseSlot::BuySet).scope.label(),
        "book / plot"
    );
    assert_eq!(Origin::Shared.label(), "MB");
    assert_eq!(Origin::Local.label(), "MT");
}
