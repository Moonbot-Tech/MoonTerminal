use moon_core::config::{HotkeysConfig, MouseGestureBinding, MoveKind};

use super::super::{
    HotkeySlot, MouseSlot, MoveKindSlot, all_mouse_slots, all_slots, mouse_slot_id,
    set_mouse_slot_verbatim, set_move_kind_slot_value, set_slot_value, slot_id,
};
use super::{Clashes, Severity};

/// A configuration with nothing bound, so each test states its own collision and no other.
fn quiet() -> HotkeysConfig {
    let mut hotkeys = HotkeysConfig::default();
    for slot in all_slots() {
        set_slot_value(&mut hotkeys, slot, String::new());
    }
    for slot in all_mouse_slots() {
        set_mouse_slot_verbatim(&mut hotkeys, slot, MouseGestureBinding::None);
    }
    hotkeys.same_hotkeys_for_move = false;
    hotkeys
}

/// The transcribed order must still match the dispatcher's own source.
///
/// [`super::RESOLVE_ORDER`] is the only thing that says which holder of a key survives, and it is a
/// COPY of `hotkeys::resolve_binding`. A copy drifts, and when this one drifted the page told
/// traders a working binding was dead. So it is checked by reading the original: every slot field
/// in the order that function tests it, with the built-in block in its real place among them.
///
/// Plausible breakage: moving one branch in `resolve_binding` — an edit that looks like tidying —
/// which silently inverts every caption on the keys either side of the move.
#[test]
fn the_transcribed_order_still_matches_the_dispatcher() {
    let source = include_str!("../../../hotkeys.rs");
    let body = source
        .split("fn resolve_binding(")
        .nth(1)
        .expect("resolve_binding")
        .split("\npub fn ")
        .next()
        .expect("its body");

    let mut at = 0usize;
    let mut previous = String::new();
    for step in super::RESOLVE_ORDER {
        let needle = match step {
            super::Step::Slot(slot) => {
                let id = slot_id(*slot);
                // A preset family is tested once, as an array, for all of its indices.
                let field = match id.rsplit_once('-') {
                    Some((head, tail)) if tail.chars().all(|c| c.is_ascii_digit()) => {
                        head.to_string()
                    }
                    _ => id,
                };
                format!("hk.{}", field.replace('-', "_"))
            }
            // A built-in is not always spelled as a keystroke in the source: the two Escape
            // branches are told apart by their MODIFIERS, so each is matched on what actually
            // distinguishes it there.
            super::Step::Builtin(strokes, _) => match strokes[0] {
                "shift-escape" => "event.modifiers.shift".to_string(),
                "escape" => "Modifiers::default()".to_string(),
                other => format!("\"{other}\""),
            },
        };
        if needle == previous {
            continue;
        }
        let found = body[at..].find(&needle).unwrap_or_else(|| {
            panic!("{needle} no longer follows the step before it — the order drifted")
        });
        at += found + needle.len();
        previous = needle;
    }
}

/// Two keyboard slots on one key: the one the dispatcher reaches first fires, the other does not,
/// and the two captions must say different things.
///
/// Handing both rows the same sentence is what the first version did, and it reported a working
/// binding as dead.
#[test]
fn the_winner_and_the_loser_are_told_different_things() {
    let _locale = crate::test_locale::force("en");
    let mut hotkeys = quiet();
    // `DrawHline` is tested before `PanicSell`: the figure slots lead the chain.
    set_slot_value(&mut hotkeys, HotkeySlot::DrawHline, "alt-6".into());
    set_slot_value(&mut hotkeys, HotkeySlot::PanicSell, "alt-6".into());
    let clashes = Clashes::build(&hotkeys);

    let winner = clashes
        .key(&hotkeys, HotkeySlot::DrawHline)
        .expect("winner");
    assert_eq!(winner.severity, Severity::Shares);
    assert!(
        winner.text.contains("Takes this binding"),
        "{}",
        winner.text
    );

    let loser = clashes.key(&hotkeys, HotkeySlot::PanicSell).expect("loser");
    assert_eq!(loser.severity, Severity::Shadowed);
    assert!(loser.text.contains("Will not fire"), "{}", loser.text);
}

/// The built-ins sit BELOW the eight figure slots and ABOVE everything else, so which side of that
/// line a slot is on decides who dies.
///
/// This is the fact the first version had backwards — backwards in a way no compiler and no green
/// build can see.
#[test]
fn a_builtin_beats_the_slots_below_it_and_loses_to_the_eight_above() {
    let mut hotkeys = quiet();
    set_slot_value(&mut hotkeys, HotkeySlot::PanicSell, "escape".into());
    set_slot_value(&mut hotkeys, HotkeySlot::FigAlert, "ctrl-shift-f10".into());
    let clashes = Clashes::build(&hotkeys);

    assert_eq!(
        clashes
            .key(&hotkeys, HotkeySlot::PanicSell)
            .expect("clash")
            .severity,
        Severity::Shadowed,
        "a trading slot cannot take Escape from the built-in"
    );
    assert_eq!(
        clashes
            .key(&hotkeys, HotkeySlot::FigAlert)
            .expect("clash")
            .severity,
        Severity::Shares,
        "a figure slot IS tested first and does take the built-in"
    );
}

/// `fig_delete` on Delete is the one pair the order calls a shadow and the runtime does not.
///
/// `Shell::on_hotkey` hands the press on to the hovered-order cancel whenever no figure is
/// selected, which is why that is the shipped default. The exemption is for THAT pair only.
#[test]
fn the_documented_delete_fallback_is_exempt_and_nothing_else_is() {
    let mut hotkeys = quiet();
    set_slot_value(&mut hotkeys, HotkeySlot::FigDelete, "delete".into());
    assert!(
        Clashes::build(&hotkeys)
            .key(&hotkeys, HotkeySlot::FigDelete)
            .is_none(),
        "the shipped default reported a clash it deliberately does not have"
    );

    set_slot_value(&mut hotkeys, HotkeySlot::FigDelete, "escape".into());
    assert!(
        Clashes::build(&hotkeys)
            .key(&hotkeys, HotkeySlot::FigDelete)
            .is_some(),
        "the same slot on Escape ends the built-in and nothing said so"
    );
}

/// Which gesture row loses depends on the BUTTON, because the handlers differ.
#[test]
fn the_button_decides_which_gesture_row_loses() {
    let _locale = crate::test_locale::force("en");
    let mut hotkeys = quiet();
    set_mouse_slot_verbatim(
        &mut hotkeys,
        MouseSlot::BuySet,
        MouseGestureBinding::MiddleAlt,
    );
    set_mouse_slot_verbatim(
        &mut hotkeys,
        MouseSlot::BuyMove,
        MouseGestureBinding::MiddleAlt,
    );
    let clashes = Clashes::build(&hotkeys);
    assert_eq!(
        clashes
            .mouse(&hotkeys, MouseSlot::BuySet)
            .expect("placement is asked first, so it takes the press")
            .severity,
        Severity::Shares
    );
    assert_eq!(
        clashes
            .mouse(&hotkeys, MouseSlot::BuyMove)
            .expect("the move row is asked second and never reached")
            .severity,
        Severity::Shadowed
    );

    // The right button carries no placement layer whatsoever.
    let mut right = quiet();
    set_mouse_slot_verbatim(&mut right, MouseSlot::BuySet, MouseGestureBinding::RightAlt);
    let clash = Clashes::build(&right)
        .mouse(&right, MouseSlot::BuySet)
        .expect("a placement gesture on the right button fires nowhere");
    assert_eq!(clash.severity, Severity::Shadowed);
    assert!(clash.text.contains("right button"), "{}", clash.text);
}

/// A move row whose kind is `None` sends nothing and does not silence the row below it.
///
/// `resolve_move_gesture` steps past such a row deliberately; counting it as a holder reported a
/// shadow that never happens.
#[test]
fn an_inert_move_row_shadows_nothing() {
    let mut hotkeys = quiet();
    set_mouse_slot_verbatim(
        &mut hotkeys,
        MouseSlot::BuyMove,
        MouseGestureBinding::Middle,
    );
    set_move_kind_slot_value(&mut hotkeys, MoveKindSlot::BuyMove, MoveKind::None);
    set_mouse_slot_verbatim(
        &mut hotkeys,
        MouseSlot::SellMove,
        MouseGestureBinding::Middle,
    );
    let clashes = Clashes::build(&hotkeys);
    assert!(
        clashes.mouse(&hotkeys, MouseSlot::SellMove).is_none(),
        "a row set to send nothing was reported as taking the press"
    );
}

/// A chart layer that answers only in a mode is shared; one that answers every press is not.
#[test]
fn a_conditional_chart_layer_shares() {
    let mut hotkeys = quiet();
    // Drawing answers Ctrl+Left only while a tool is armed, and it is asked FIRST.
    set_mouse_slot_verbatim(
        &mut hotkeys,
        MouseSlot::BuySet,
        MouseGestureBinding::LeftCtrl,
    );
    // The X-scale sync is asked LAST, so this row keeps firing and the sync is what dies.
    set_mouse_slot_verbatim(
        &mut hotkeys,
        MouseSlot::ShortSet,
        MouseGestureBinding::MiddleShift,
    );
    let clashes = Clashes::build(&hotkeys);
    assert_eq!(
        clashes
            .mouse(&hotkeys, MouseSlot::BuySet)
            .expect("drawing shares this gesture")
            .severity,
        Severity::Shares
    );
    assert_eq!(
        clashes
            .mouse(&hotkeys, MouseSlot::ShortSet)
            .expect("the sync sits below this row")
            .severity,
        Severity::Shares
    );
}

/// The two halves of one move row holding one gesture is not a clash at any setting.
#[test]
fn the_two_halves_of_one_move_row_are_not_a_clash() {
    let mut hotkeys = quiet();
    set_mouse_slot_verbatim(
        &mut hotkeys,
        MouseSlot::BuyMove,
        MouseGestureBinding::MiddleCtrl,
    );
    set_mouse_slot_verbatim(
        &mut hotkeys,
        MouseSlot::ShortBuyMove,
        MouseGestureBinding::MiddleCtrl,
    );
    let clashes = Clashes::build(&hotkeys);
    assert!(clashes.mouse(&hotkeys, MouseSlot::BuyMove).is_none());
}

/// The shipped defaults must leave no row dead.
#[test]
fn the_shipped_defaults_leave_nothing_dead() {
    let hotkeys = HotkeysConfig::default();
    let clashes = Clashes::build(&hotkeys);

    let dead: Vec<String> = all_slots()
        .into_iter()
        .filter(|slot| {
            clashes
                .key(&hotkeys, *slot)
                .is_some_and(|c| c.severity == Severity::Shadowed)
        })
        .map(slot_id)
        .collect();
    assert!(dead.is_empty(), "a shipped key never fires: {dead:?}");

    let dead_mice: Vec<&str> = all_mouse_slots()
        .into_iter()
        .filter(|slot| {
            clashes
                .mouse(&hotkeys, *slot)
                .is_some_and(|c| c.severity == Severity::Shadowed)
        })
        .map(mouse_slot_id)
        .collect();
    assert!(
        dead_mice.is_empty(),
        "a shipped gesture never fires: {dead_mice:?}"
    );
}
