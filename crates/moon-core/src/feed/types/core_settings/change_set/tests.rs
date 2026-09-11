use super::CoreChangeSet;
use crate::feed::types::core_settings::tests::wire_default as base;
use crate::feed::{CORE_FIELDS, CoreConfig, FieldMask, MoveRow, index_of};

fn index(key: &str) -> usize {
    index_of(key).unwrap_or_else(|| panic!("{key} is in the table"))
}

fn keys(set: &CoreChangeSet) -> Vec<&'static str> {
    set.fields().iter().map(|&i| CORE_FIELDS[i].key).collect()
}

/// A set seeded from a page, ready for edits measured against that page.
fn seeded(page: &CoreConfig) -> CoreChangeSet {
    let mut set = CoreChangeSet::default();
    set.seed(page);
    set
}

/// One edit stages the field it moved; moving it back to the base keeps it staged, with the value
/// now on the page — the set is written to cores whose base is not this one.
#[test]
fn a_field_moved_away_and_back_stays_staged_with_the_page_value() {
    let base = base();
    let mut set = seeded(&base);
    let mut page = base.clone();
    page.general.take_profit_pct += 1.0;
    set.note_edit(&page, &base);
    assert_eq!(set.fields(), &[index("general.take_profit_pct")]);
    assert_eq!(set.len(), 1);

    page.general.take_profit_pct = base.general.take_profit_pct;
    set.note_edit(&page, &base);
    assert_eq!(set.len(), 1);
    let mut other = base.clone();
    other.general.take_profit_pct = 42.0;
    set.overlay(&mut other);
    assert_eq!(other.general.take_profit_pct, base.general.take_profit_pct);
    assert_eq!(set.mask(), FieldMask::EMPTY.with_general());
}

/// An edit that moves nothing away from the base stages nothing: a dead row's slider, a stepper
/// pressed at its floor, a value typed over itself. And with no page seeded there is nothing to
/// measure, so nothing is staged either.
#[test]
fn an_edit_that_lands_on_the_base_stages_nothing() {
    let base = base();
    let mut set = seeded(&base);
    set.note_edit(&base, &base);
    assert!(set.is_empty());
    assert_eq!(set.mask(), FieldMask::EMPTY);

    let mut unseeded = CoreChangeSet::default();
    let mut page = base.clone();
    page.general.take_profit_pct += 1.0;
    unseeded.note_edit(&page, &base);
    assert!(unseeded.is_empty());
}

/// An edit on one field must not un-stage another: membership is decided per touched field.
#[test]
fn an_edit_elsewhere_keeps_earlier_changes() {
    let base = base();
    let mut set = seeded(&base);
    let mut page = base.clone();
    page.general.take_profit_pct += 1.0;
    set.note_edit(&page, &base);
    page.special.log_level += 1;
    set.note_edit(&page, &base);
    assert_eq!(
        keys(&set),
        vec!["general.take_profit_pct", "special.log_level"]
    );
    assert!(set.contains(index("special.log_level")));
    assert_eq!(set.mask(), FieldMask::EMPTY.with_general().with_special());
}

/// The overlay carries exactly the staged fields onto a foreign snapshot — the core's own values
/// survive everywhere else, which is what makes a bulk OK safe over cores that differ.
#[test]
fn overlay_writes_only_the_staged_fields() {
    let base = base();
    let mut set = seeded(&base);
    let mut page = base.clone();
    page.general.take_profit_pct = 9.5;
    set.note_edit(&page, &base);

    let mut other = base.clone();
    other.special.log_level = 4;
    other.general.trailing_pct = 3.0;
    set.overlay(&mut other);
    assert_eq!(other.general.take_profit_pct, 9.5);
    assert_eq!(other.special.log_level, 4);
    assert_eq!(other.general.trailing_pct, 3.0);
}

/// The case the latch exists for: a change staged on core A, then the same field moved on core
/// B's page to the value B already holds, is still a change for A.
#[test]
fn a_change_re_edited_onto_another_cores_base_survives() {
    let base_a = base();
    let mut set = seeded(&base_a);
    let mut page_a = base_a.clone();
    page_a.general.take_profit_pct = 6.0;
    set.note_edit(&page_a, &base_a);

    // Core B holds 6.0 already; its page seeds as its base with the change laid over.
    let mut base_b = base();
    base_b.general.take_profit_pct = 6.0;
    let mut page_b = base_b.clone();
    set.overlay(&mut page_b);
    set.seed(&page_b);
    // The user nudges the value to 6.5 and back to 6.0 on B's page.
    page_b.general.take_profit_pct = 6.5;
    set.note_edit(&page_b, &base_b);
    page_b.general.take_profit_pct = 6.0;
    set.note_edit(&page_b, &base_b);
    assert_eq!(set.len(), 1);
    let mut send_a = base_a.clone();
    set.overlay(&mut send_a);
    assert_eq!(send_a.general.take_profit_pct, 6.0);
}

/// A decision on a mixed control stages the field even when the page's value did not move, and
/// stages it AT that value; a derived field is not a decision.
#[test]
fn stage_takes_the_page_value_whether_or_not_it_moved() {
    let base = base();
    let mut set = seeded(&base);
    let tp_on = index("general.take_profit_on");
    set.stage(tp_on, &base);
    assert_eq!(set.fields(), &[tp_on]);
    let mut other = base.clone();
    other.general.take_profit_on = !base.general.take_profit_on;
    set.overlay(&mut other);
    assert_eq!(other.general.take_profit_on, base.general.take_profit_on);

    let mut mirrored = base.clone();
    mirrored.gestures.same_hotkeys_for_move = true;
    let mut set = seeded(&mirrored);
    set.stage(index("gestures.short_buy_move_click"), &mirrored);
    assert!(set.is_empty(), "a copy is not a decision");
    let mut unseeded = CoreChangeSet::default();
    unseeded.stage(tp_on, &base);
    assert!(unseeded.is_empty());
}

/// `restart` forgets the fields and measures the next edit against the page it was given.
#[test]
fn restart_forgets_fields_and_rebases() {
    let base = base();
    let mut set = seeded(&base);
    let mut page = base.clone();
    page.general.take_profit_pct = 9.5;
    set.note_edit(&page, &base);
    assert_eq!(set.len(), 1);
    set.restart(&page);
    assert!(set.is_empty());
    // The sent page is the new base: putting the same value again is no edit, the next one is.
    let rebased = page.clone();
    set.note_edit(&page, &rebased);
    assert!(set.is_empty());
    page.general.take_profit_pct = 10.0;
    set.note_edit(&page, &rebased);
    assert_eq!(set.len(), 1);
}

/// An empty set overlays nothing and clears cleanly.
#[test]
fn empty_set_is_inert() {
    let base = base();
    let mut cfg = base.clone();
    CoreChangeSet::default().overlay(&mut cfg);
    assert_eq!(cfg, base);

    let mut set = seeded(&base);
    let mut page = base.clone();
    page.general.take_profit_pct += 1.0;
    set.note_edit(&page, &base);
    set.clear();
    assert!(set.is_empty());
    let mut cfg = base.clone();
    set.overlay(&mut cfg);
    assert_eq!(cfg, base);
}

/// With the mirror ON, a long-gesture edit moves its short twin on the page too — but the twin is
/// a copy, not a change: it is not staged, so a target whose mirror is OFF keeps its own short,
/// while a target whose mirror is ON follows the new long.
#[test]
fn mirrored_short_gestures_are_derived_not_staged() {
    let mut base_a = base();
    base_a.gestures.same_hotkeys_for_move = true;
    base_a.gestures.buy_move_click = 1;
    base_a.gestures.short_buy_move_click = 1;
    let mut set = seeded(&base_a);
    let mut page_a = base_a.clone();
    page_a
        .gestures
        .set_move_gesture(MoveRow::OpenPrimary, false, 4);
    assert_eq!(page_a.gestures.short_buy_move_click, 4, "the page mirrors");
    set.note_edit(&page_a, &base_a);
    assert_eq!(keys(&set), vec!["gestures.buy_move_click"]);

    let mut independent = base();
    independent.gestures.same_hotkeys_for_move = false;
    independent.gestures.buy_move_click = 7;
    independent.gestures.short_buy_move_click = 9;
    set.overlay(&mut independent);
    assert_eq!(independent.gestures.buy_move_click, 4);
    assert_eq!(
        independent.gestures.short_buy_move_click, 9,
        "its own short survives"
    );

    let mut mirrored = base();
    mirrored.gestures.same_hotkeys_for_move = true;
    mirrored.gestures.buy_move_click = 7;
    mirrored.gestures.short_buy_move_click = 7;
    set.overlay(&mut mirrored);
    assert_eq!(mirrored.gestures.buy_move_click, 4);
    assert_eq!(mirrored.gestures.short_buy_move_click, 4);
}

/// Turning the mirror ON stages the flag and nothing of the anchor's copies; a target ends up
/// mirroring ITS OWN longs.
#[test]
fn turning_the_mirror_on_stages_the_flag_alone() {
    let mut base_a = base();
    base_a.gestures.same_hotkeys_for_move = false;
    base_a.gestures.buy_move_click = 5;
    base_a.gestures.short_buy_move_click = 6;
    let mut set = seeded(&base_a);
    let mut page_a = base_a.clone();
    page_a.gestures.set_same_hotkeys(true);
    set.note_edit(&page_a, &base_a);
    assert_eq!(keys(&set), vec!["gestures.same_hotkeys_for_move"]);

    let mut target = base();
    target.gestures.same_hotkeys_for_move = false;
    target.gestures.buy_move_click = 8;
    target.gestures.short_buy_move_click = 9;
    set.overlay(&mut target);
    assert!(target.gestures.same_hotkeys_for_move);
    assert_eq!(target.gestures.buy_move_click, 8);
    assert_eq!(target.gestures.short_buy_move_click, 8);
}

/// Turning the mirror OFF makes the shorts the user's own: the copy the page has been showing is
/// then a change against the base if it differs, exactly as if it had been typed.
#[test]
fn turning_the_mirror_off_stages_the_shorts_the_page_shows() {
    let mut base_a = base();
    base_a.gestures.same_hotkeys_for_move = true;
    base_a.gestures.buy_move_click = 1;
    base_a.gestures.short_buy_move_click = 1;
    let mut set = seeded(&base_a);
    let mut page = base_a.clone();
    page.gestures
        .set_move_gesture(MoveRow::OpenPrimary, false, 4);
    set.note_edit(&page, &base_a);
    page.gestures.set_same_hotkeys(false);
    set.note_edit(&page, &base_a);
    let mut staged = keys(&set);
    staged.sort_unstable();
    assert_eq!(
        staged,
        vec![
            "gestures.buy_move_click",
            "gestures.same_hotkeys_for_move",
            "gestures.short_buy_move_click",
        ]
    );
}

/// A short gesture staged on a core whose mirror is OFF survives a visit to a core whose mirror
/// is ON — where the page shows it as the long twin's copy — and is still written back to the
/// first core.
#[test]
fn a_staged_short_survives_a_mirrored_anchor() {
    let mut base_a = base();
    base_a.gestures.same_hotkeys_for_move = false;
    base_a.gestures.buy_move_click = 1;
    base_a.gestures.short_buy_move_click = 2;
    let mut set = seeded(&base_a);
    let mut page_a = base_a.clone();
    page_a
        .gestures
        .set_move_gesture(MoveRow::OpenPrimary, true, 5);
    set.note_edit(&page_a, &base_a);
    assert_eq!(keys(&set), vec!["gestures.short_buy_move_click"]);

    // Core B mirrors: its page shows the short as its long, whatever was staged.
    let mut base_b = base();
    base_b.gestures.same_hotkeys_for_move = true;
    base_b.gestures.buy_move_click = 7;
    base_b.gestures.short_buy_move_click = 7;
    let mut page_b = base_b.clone();
    set.overlay(&mut page_b);
    assert_eq!(page_b.gestures.short_buy_move_click, 7, "derived on B");
    set.seed(&page_b);
    // An unrelated edit on B must not disturb the staged short either.
    page_b.general.take_profit_pct += 1.0;
    set.note_edit(&page_b, &base_b);
    assert!(set.contains(index("gestures.short_buy_move_click")));

    // Turning B's mirror off makes the short a value of its own again: the page then shows the
    // STAGED value, not B's copy, once the set is laid back over it — which is what OK sends.
    page_b.gestures.set_same_hotkeys(false);
    set.note_edit(&page_b, &base_b);
    set.overlay(&mut page_b);
    assert_eq!(page_b.gestures.short_buy_move_click, 5);

    let mut send_a = base_a.clone();
    set.overlay(&mut send_a);
    assert_eq!(send_a.gestures.short_buy_move_click, 5);
}

/// Regression target (2026-09-11): a short gesture that STOPS being a copy and is thereby staged
/// for the first time must take the value the page shows — the long twin's, which is what the
/// user set and what the page still reads — not whatever the shadow held from before the mirror.
/// It read as "was a copy" by membership taken AFTER this edit's own insertion, so the shadow kept
/// the pre-mirror value and OK wrote it to every core while the page showed another.
#[test]
fn a_short_first_staged_by_the_mirror_going_off_takes_the_page_value() {
    let mut base_a = base();
    base_a.gestures.same_hotkeys_for_move = true;
    base_a.gestures.buy_move_click = 1;
    base_a.gestures.short_buy_move_click = 1;
    let mut set = seeded(&base_a);
    let mut page = base_a.clone();
    page.gestures
        .set_move_gesture(MoveRow::OpenPrimary, false, 4);
    set.note_edit(&page, &base_a);
    assert_eq!(page.gestures.short_buy_move_click, 4, "the page mirrors");
    page.gestures.set_same_hotkeys(false);
    set.note_edit(&page, &base_a);

    let mut send = base_a.clone();
    set.overlay(&mut send);
    assert_eq!(
        send.gestures.short_buy_move_click, 4,
        "the short is written at the value the page showed when the mirror went off"
    );
}
