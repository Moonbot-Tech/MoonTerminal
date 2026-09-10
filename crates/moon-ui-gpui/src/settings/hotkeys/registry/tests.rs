use std::collections::HashSet;

use moon_core::config::{GestureSlot, HotkeysConfig, KeySlot, MANUAL_STRATEGY_KEYS, MoveKindSlot};
use moon_core::feed::{CoreHotkeyAction, CoreHotkeyLayout, GestureSettings};

use super::super::pull::preview_core_hotkeys;
use super::super::pull_gestures::{GestureTarget, preview_core_gestures};
use super::*;
use crate::hotkeys::meta::{Origin, Scope};

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

/// Every slot the config holds is on the page exactly once — the property that makes this list a
/// registry rather than one more copy.
///
/// Checked against the config's own lists, not against this one: a slot added to `KeySlot` and
/// forgotten here is a field the user can no longer edit, and nothing else would say so.
#[test]
fn every_slot_is_on_the_page_exactly_once() {
    let keys: Vec<KeySlot> = slots().filter_map(|spec| spec.key()).collect();
    let gestures: Vec<GestureSlot> = slots().filter_map(|spec| spec.mouse()).collect();

    for slot in KeySlot::all() {
        let n = keys.iter().filter(|k| **k == slot).count();
        assert_eq!(n, 1, "{slot:?} is on the page {n} times");
    }
    for slot in GestureSlot::ALL {
        let n = gestures.iter().filter(|g| **g == slot).count();
        assert_eq!(n, 1, "{slot:?} is on the page {n} times");
    }
    // And the page carries nothing the config does not — a row whose slot the file cannot hold.
    assert_eq!(keys.len(), KeySlot::all().len());
    assert_eq!(gestures.len(), GestureSlot::ALL.len());
}

/// Every kind selector sits on the row of the gesture it belongs to, and the short rows carry
/// none: the page draws a kind column on the four long move rows and greys the short ones out,
/// which is Moonbot's own single-kind-per-row layout.
#[test]
fn the_kind_selectors_sit_on_the_long_move_rows() {
    let with_kind: Vec<(GestureSlot, MoveKindSlot)> = slots()
        .filter_map(|spec| Some((spec.mouse()?, spec.kind()?)))
        .collect();
    assert_eq!(with_kind.len(), MoveKindSlot::ALL.len());
    for (gesture, kind) in with_kind {
        assert_eq!(kind.half(false), gesture, "{kind:?} is on the wrong row");
    }
    for spec in slots().filter(SlotSpec::follows_mirror) {
        assert_eq!(
            spec.kind(),
            None,
            "{}: a short row carries no kind",
            spec.title()
        );
    }
}

/// Element ids are how the page tells its inputs apart; two editors with one id share one state.
///
/// The ids are derived from the stems, so this is really a check that no two slots share a stem —
/// which nothing in `moon_core` asserts, because the stem is only ever dressed up here.
#[test]
fn editor_ids_are_unique() {
    let mut seen = HashSet::new();
    for slot in KeySlot::all() {
        assert!(
            seen.insert(key_id(slot)),
            "{} names two key editors",
            key_id(slot)
        );
    }
    let mut seen = HashSet::new();
    for slot in GestureSlot::ALL {
        assert!(
            seen.insert(gesture_id(slot)),
            "{} names two gesture editors",
            gesture_id(slot)
        );
    }
}

/// Every title and hint resolves to real text, in the locale whose echo is easiest to tell apart.
///
/// The ids and locale keys are DERIVED from the slot stems, which is what makes this worth a test:
/// nothing else checks that `hotkeys.<stem>_hint` exists for a new slot. rust-i18n answers an
/// unknown key with `"<locale>.<key>"`, so an echo still contains the `hotkeys.` prefix.
#[test]
fn every_row_names_itself_in_words() {
    let _locale = crate::test_locale::force("en");
    for spec in slots() {
        for text in [spec.title(), spec.hint()] {
            assert!(
                !text.is_empty() && !text.contains("hotkeys."),
                "{}: a locale key did not resolve: {text:?}",
                spec.title()
            );
        }
    }
    // The titles a caption or a preview line uses to name a slot are the row's own.
    for spec in slots() {
        if let Some(key) = spec.key() {
            assert_eq!(key_title(key), spec.title());
        }
        if let Some(mouse) = spec.mouse() {
            assert_eq!(gesture_title(mouse), spec.title());
        }
    }
}

/// The merged figure-delete row: one title, both editors, and marks that read as one row.
///
/// Plausible breakage: the row split back into two, or a merged scope that lists the pointer twice
/// ("cursor / figure") because the join stopped absorbing the narrower place.
#[test]
fn the_figure_delete_row_carries_both_editors_and_one_set_of_marks() {
    let _locale = crate::test_locale::force("en");
    let spec = slots()
        .find(|spec| spec.key() == Some(KeySlot::FigDelete))
        .expect("the figure-delete key is on the page");
    assert_eq!(spec.mouse(), Some(GestureSlot::FigDelete));
    assert_eq!(spec.group, HotkeyGroup::Draw);
    let marks = spec.meta();
    assert_eq!(marks.origin, Origin::Local);
    assert!(marks.scope.intersects(Scope::SELECTION));
    assert!(marks.scope.intersects(Scope::CURSOR));
    assert!(
        !marks.scope.intersects(Scope::FIGURE),
        "the figure under the pointer is a place the pointer is, and the mark already says cursor"
    );
    assert_eq!(marks.scope.label(), "cursor / selection");
}

/// A row with two editors carries ONE origin mark, so its halves must not disagree about it.
///
/// This is the check that forces a decision when the first key-that-travels meets a
/// gesture-that-does-not on one row: the mark cannot answer for both, and the row will need a mark
/// per half rather than a silent choice.
#[test]
fn the_halves_of_a_two_editor_row_agree_on_origin() {
    for spec in slots() {
        if let (Some(key), Some(mouse)) = (spec.key(), spec.mouse()) {
            assert_eq!(
                meta::key_slot_meta(key).origin,
                meta::gesture_slot_meta(mouse).origin,
                "{}: one mark cannot say what a paste does to two halves that differ",
                spec.title()
            );
        }
    }
}

/// The placement rows appear on the page in the order a press is tried against them.
///
/// `placement_intent` walks `GestureSlot::ALL` and the first match fires; `clash::same_layer_rank`
/// reads the same list to say which of two rows holding one gesture wins. A page that listed them
/// in another order would show the loser above the winner.
#[test]
fn the_placement_rows_are_listed_in_dispatch_order() {
    let on_page: Vec<GestureSlot> = slots()
        .filter_map(|spec| spec.mouse())
        .filter(|slot| slot.placement().is_some())
        .collect();
    let dispatched: Vec<GestureSlot> = GestureSlot::ALL
        .into_iter()
        .filter(|slot| slot.placement().is_some())
        .collect();
    assert_eq!(on_page, dispatched);
}

/// The slot ids a "pull layout from core" would actually reconcile, taken from the pull itself.
///
/// Built through [`preview_core_hotkeys`] rather than through its inner action-to-slot map: the
/// preview is what the button runs, it adds the three preset families the map never sees, and
/// going through it keeps the mapping private.
fn slots_the_pull_writes() -> Vec<KeySlot> {
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
    .map(|row| row.slot)
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
    for spec in slots() {
        let Some(key) = spec.key() else { continue };
        assert_eq!(
            meta::key_slot_meta(key).origin == Origin::Shared,
            written.contains(&key),
            "{}: the mark and the pull disagree about whether this row is overwritten",
            spec.title()
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
    for spec in slots() {
        let Some(mouse) = spec.mouse() else { continue };
        let marked = meta::gesture_slot_meta(mouse).origin == Origin::Shared;
        let carried = written.contains(&GestureTarget::Gesture(mouse));
        assert_eq!(
            marked,
            carried,
            "{}: the mark and the gesture pull disagree about whether this row is overwritten",
            spec.title()
        );
    }
    // The two rows that are not slots and so have no meta of their own. Both are written by the
    // pull, so both must carry the mark that says so — the mirror switch especially, since it
    // decides whether the four short rows follow the long ones at all.
    assert!(written.contains(&GestureTarget::SameForMove));
    assert_eq!(meta::SAME_FOR_MOVE.origin, Origin::Shared);
    for row in MoveKindSlot::ALL {
        assert!(
            written.contains(&GestureTarget::Kind(row)),
            "{row:?} is no longer pulled, but its gesture row still claims Moonbot writes it"
        );
    }
}
