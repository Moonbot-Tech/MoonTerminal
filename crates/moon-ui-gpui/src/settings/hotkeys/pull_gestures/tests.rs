use moon_core::config::{HotkeysConfig, MouseGestureBinding, MoveKind};
use moon_core::feed::GestureSettings;

use super::*;

/// A core layout with everything switched off, to be filled per test.
fn empty_core() -> GestureSettings {
    GestureSettings {
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
    }
}

/// The wire ordinal of a gesture, which is its index in the terminal's own list.
fn ordinal(gesture: MouseGestureBinding) -> u8 {
    MouseGestureBinding::ALL
        .iter()
        .position(|g| *g == gesture)
        .expect("gesture is in ALL") as u8
}

fn row(rows: &[GesturePullRow], target: GestureTarget) -> &GesturePullRow {
    rows.iter()
        .find(|r| r.target == target)
        .unwrap_or_else(|| panic!("no row for {target:?}"))
}

/// The pull must read a SHORT row through the core's own resolution, not off its short field.
///
/// With `same_hotkeys_for_move` set the core acts on the long gesture and its short field may hold
/// anything — Moonbot repairs that field only when the row is edited. Reading the field would
/// import a binding the core never fires, and the trader would then wonder why their terminal and
/// their bot disagree on a short position.
///
/// Plausible breakage: replacing `move_gesture(row, true)` with `g.short_buy_move_click`.
#[test]
fn a_mirrored_short_row_takes_the_long_gesture_not_the_stale_short_field() {
    let mut core = empty_core();
    core.same_hotkeys_for_move = true;
    core.buy_move_click = ordinal(MouseGestureBinding::MiddleCtrl);
    // Stale, and never what this core acts on.
    core.short_buy_move_click = ordinal(MouseGestureBinding::RightAlt);

    let rows = preview_core_gestures(&HotkeysConfig::default(), &core);
    let short = row(&rows, GestureTarget::Gesture(MouseSlot::ShortBuyMove));
    assert_eq!(
        short.new,
        Some(GestureValue::Gesture(MouseGestureBinding::MiddleCtrl)),
        "the short row imported the stale field instead of what the core fires"
    );
}

/// A long row must not overwrite a short value the preview decided to leave alone.
///
/// The setup is ordinary: the terminal mirrors, the core does not. The editor's setter would carry
/// the long write onto the short twin, and the short row — `Unchanged`, so skipped — would never
/// repair it. That is why the apply writes verbatim.
///
/// Plausible breakage: `apply_core_gestures` calling `set_mouse_slot_value`.
#[test]
fn applying_a_long_gesture_leaves_the_short_row_the_core_did_not_change() {
    let mut hotkeys = HotkeysConfig::default();
    hotkeys.same_hotkeys_for_move = true;
    hotkeys.buy_move_click = MouseGestureBinding::LeftDouble;
    hotkeys.short_buy_move_click = MouseGestureBinding::RightShift;

    let mut core = empty_core();
    core.same_hotkeys_for_move = false;
    core.buy_move_click = ordinal(MouseGestureBinding::MiddleCtrl);
    core.short_buy_move_click = ordinal(MouseGestureBinding::RightShift);

    let rows = preview_core_gestures(&hotkeys, &core);
    assert!(apply_core_gestures(&mut hotkeys, &rows));
    assert_eq!(hotkeys.buy_move_click, MouseGestureBinding::MiddleCtrl);
    assert_eq!(
        hotkeys.short_buy_move_click,
        MouseGestureBinding::RightShift,
        "the long row mirrored over a short value the core agreed with"
    );
    assert!(!hotkeys.same_hotkeys_for_move);
}

/// An ordinal past the end of the list is a core this build does not understand: the row says so
/// and writes nothing, rather than falling back to the first gesture in the list.
///
/// Plausible breakage: `ALL[raw as usize]` instead of `ALL.get(..)` — a panic in the settings
/// window — or a `unwrap_or_default()` that silently disarms the row.
#[test]
fn an_unknown_ordinal_is_refused_rather_than_guessed() {
    let mut hotkeys = HotkeysConfig::default();
    hotkeys.buy_set_click = MouseGestureBinding::LeftDouble;
    // The flag travels too, and it is not what this test is about.
    hotkeys.same_hotkeys_for_move = false;
    let mut core = empty_core();
    core.buy_set_click = 200;
    core.replace_buy_kind = 99;

    let rows = preview_core_gestures(&hotkeys, &core);
    let gesture = row(&rows, GestureTarget::Gesture(MouseSlot::BuySet));
    assert_eq!(gesture.verdict, PullVerdict::Unsupported);
    assert_eq!(gesture.new, None);
    let kind = row(&rows, GestureTarget::Kind(MoveKindSlot::BuyMove));
    assert_eq!(kind.verdict, PullVerdict::Unsupported);

    apply_core_gestures(&mut hotkeys, &rows);
    assert_eq!(
        hotkeys.buy_set_click,
        MouseGestureBinding::LeftDouble,
        "an ordinal this build cannot read was written anyway"
    );
    assert_eq!(hotkeys.buy_move_kind, MoveKind::ParallelShift);
}

/// A zero ordinal is a VALUE, so a core that holds `None` there hands `None` over.
///
/// This is what pulling a LAYOUT means, and the alternative was tried: skipping zero kept a local
/// gesture the core had switched off, and it could not even be applied per field — moonproto ships
/// `replace_buy_kind: 3` beside `replace_buy_kind_2: 0`, so "is zero the default" differs inside one
/// block. The terminal never sees an unfilled block anyway.
///
/// Plausible breakage: reintroducing a "skip the zero, it is probably unset" arm — which would also
/// make a stock core silently leave the secondary move rows on whatever the terminal had.
#[test]
fn a_core_value_of_none_arrives_as_none() {
    let mut hotkeys = HotkeysConfig::default();
    hotkeys.buy_set_click = MouseGestureBinding::LeftDouble;
    hotkeys.buy_move_kind = MoveKind::ParallelShift;
    hotkeys.buy_move_kind2 = MoveKind::ParallelShift;
    hotkeys.same_hotkeys_for_move = false;

    let rows = preview_core_gestures(&hotkeys, &empty_core());
    assert_eq!(
        row(&rows, GestureTarget::Gesture(MouseSlot::BuySet)).verdict,
        PullVerdict::WillApply
    );
    assert!(apply_core_gestures(&mut hotkeys, &rows));
    assert_eq!(hotkeys.buy_set_click, MouseGestureBinding::None);
    // Both kind fields, primary and secondary: the rule is one rule.
    assert_eq!(hotkeys.buy_move_kind, MoveKind::None);
    assert_eq!(hotkeys.buy_move_kind2, MoveKind::None);
}

/// Flipping the mirror OFF must re-aim the four short rows, even where their own verdict said
/// nothing changed.
///
/// With the flag still on, a short row's "current" is the LONG value — so it can equal what the core
/// holds and score `Unchanged` while the short FIELD holds something else entirely. Turn the flag
/// off and that stale field is what fires. A second pull cannot repair it: the row reads `Unchanged`
/// again.
///
/// Plausible breakage: repairing the short rows only when the flag turns ON.
#[test]
fn turning_the_mirror_off_re_aims_the_short_rows_it_makes_live() {
    let mut hotkeys = HotkeysConfig::default();
    hotkeys.same_hotkeys_for_move = true;
    hotkeys.buy_move_click = MouseGestureBinding::LeftDouble;
    hotkeys.short_buy_move_click = MouseGestureBinding::RightAlt;

    let mut core = empty_core();
    core.same_hotkeys_for_move = false;
    core.buy_move_click = ordinal(MouseGestureBinding::MiddleCtrl);
    // Equal to the terminal's mirror-resolved "current", so the short row scores Unchanged.
    core.short_buy_move_click = ordinal(MouseGestureBinding::LeftDouble);

    let rows = preview_core_gestures(&hotkeys, &core);
    assert_eq!(
        row(&rows, GestureTarget::Gesture(MouseSlot::ShortBuyMove)).verdict,
        PullVerdict::Unchanged
    );
    assert!(apply_core_gestures(&mut hotkeys, &rows));
    assert!(!hotkeys.same_hotkeys_for_move);
    assert_eq!(
        hotkeys.short_buy_move_click,
        MouseGestureBinding::LeftDouble,
        "the stale short field survived the flag going off and is now what fires"
    );
}

/// Flipping the mirror ON re-aims them the other way, so the pair cannot sit divergent under a flag
/// that makes the short field unreachable.
#[test]
fn turning_the_mirror_on_re_aims_the_short_rows_it_hides() {
    let mut hotkeys = HotkeysConfig::default();
    hotkeys.same_hotkeys_for_move = false;
    hotkeys.short_buy_move_click = MouseGestureBinding::RightAlt;

    let mut core = empty_core();
    core.same_hotkeys_for_move = true;
    core.buy_move_click = ordinal(MouseGestureBinding::MiddleCtrl);

    let rows = preview_core_gestures(&hotkeys, &core);
    assert!(apply_core_gestures(&mut hotkeys, &rows));
    assert!(hotkeys.same_hotkeys_for_move);
    assert_eq!(hotkeys.buy_move_click, MouseGestureBinding::MiddleCtrl);
    assert_eq!(
        hotkeys.short_buy_move_click,
        MouseGestureBinding::MiddleCtrl
    );
}

/// Every field the preview offers is a field the apply actually writes.
///
/// Plausible breakage: a new gesture row added to the preview with no arm in the apply — it would
/// show "will apply" and then quietly do nothing.
#[test]
fn every_row_the_preview_offers_is_one_the_apply_writes() {
    let mut hotkeys = HotkeysConfig::default();
    let mut core = empty_core();
    // Something different from the shipped defaults in every field.
    let g = ordinal(MouseGestureBinding::MiddleAlt);
    core.buy_set_click = g;
    core.short_set_click = g;
    core.pending_order_set_click = g;
    core.pending_short_set_click = g;
    core.buy_move_click = g;
    core.short_buy_move_click = ordinal(MouseGestureBinding::RightCtrl);
    core.sell_move_click = g;
    core.short_sell_move_click = ordinal(MouseGestureBinding::RightCtrl);
    core.buy_move_click_2 = g;
    core.short_buy_move_click_2 = ordinal(MouseGestureBinding::RightCtrl);
    core.sell_move_click_2 = g;
    core.short_sell_move_click_2 = ordinal(MouseGestureBinding::RightCtrl);
    core.replace_buy_kind = 3;
    core.replace_sell_kind = 3;
    core.replace_buy_kind_2 = 3;
    core.replace_sell_kind_2 = 3;
    core.same_hotkeys_for_move = true;

    let rows = preview_core_gestures(&hotkeys, &core);
    assert!(apply_core_gestures(&mut hotkeys, &rows));

    // Re-previewing the applied set must find nothing left to do.
    let after = preview_core_gestures(&hotkeys, &core);
    let unapplied: Vec<_> = after
        .iter()
        .filter(|r| r.verdict == PullVerdict::WillApply)
        .map(|r| r.target)
        .collect();
    assert!(
        unapplied.is_empty(),
        "these rows offered a change the apply never wrote: {unapplied:?}"
    );
    assert!(hotkeys.same_hotkeys_for_move);
    assert_eq!(hotkeys.buy_move_kind, MoveKind::ALL[3]);
}

/// Building a preview must not touch the draft — the user has not pressed anything yet.
#[test]
fn building_a_preview_changes_nothing() {
    let hotkeys = HotkeysConfig::default();
    let before = hotkeys.clone();
    let mut core = empty_core();
    core.buy_set_click = ordinal(MouseGestureBinding::MiddleAlt);
    let _ = preview_core_gestures(&hotkeys, &core);
    assert_eq!(before.buy_set_click, hotkeys.buy_set_click);
}
