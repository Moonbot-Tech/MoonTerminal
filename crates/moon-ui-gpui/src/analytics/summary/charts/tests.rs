//! Unit tests for the Summary chart chrome: hover fences, popup limits and core colours.

use super::hover::enter_decision;
use super::{PopupHover, PopupKey, distinct_core_colors, popup_limits, popup_outer_width};

/// Adapt the production entry decision to test selection and delayed-work revision tokens.
fn model_chart_enter(
    hover: &mut PopupHover,
    selected: &mut Option<PopupKey>,
    key: PopupKey,
    from_popup: bool,
) -> Option<u64> {
    if let Some(revision) = enter_decision(hover, *selected, key, from_popup) {
        Some(revision)
    } else {
        *selected = Some(key);
        None
    }
}

/// Compare both pointer ownership and the highlight projection select_popup writes.
fn assert_popup_selection_equal(
    old: &PopupHover,
    new: &PopupHover,
    old_selected: Option<PopupKey>,
    new_selected: Option<PopupKey>,
) {
    assert_eq!(old.target, new.target);
    assert_eq!(old.over_popup, new.over_popup);
    let highlights = |key: Option<PopupKey>| key.map(|PopupKey::Profit(i)| i);
    assert_eq!(highlights(old_selected), highlights(new_selected));
}

/// Omitting old source leave must still fence its timer and preserve neighbour dwell in either direction.
#[test]
fn profit_single_source_matches_old_column_switches_both_directions() {
    for (a, b) in [(2, 7), (7, 2)] {
        for leave_first in [true, false] {
            let (first, next) = (PopupKey::Profit(a), PopupKey::Profit(b));
            let (mut old, mut new) = (PopupHover::default(), PopupHover::default());
            let (mut old_selected, mut new_selected) = (None, None);
            model_chart_enter(&mut old, &mut old_selected, first, false);
            new.profit_pointer_entered();
            assert!(new.profit_move_should_enter(a));
            model_chart_enter(&mut new, &mut new_selected, first, false);
            let stale = leave_first.then(|| old.leave(first, false)).flatten();
            let old_reveal = model_chart_enter(&mut old, &mut old_selected, next, false).unwrap();
            if !leave_first {
                assert_eq!(old.leave(first, false), None);
            }
            assert!(new.profit_move_should_enter(b));
            let new_reveal = model_chart_enter(&mut new, &mut new_selected, next, false).unwrap();
            assert_popup_selection_equal(&old, &new, old_selected, new_selected);
            if let Some(revision) = stale {
                assert!(!old.expire(first, revision));
            }
            assert!(old.is_current(next, old_reveal));
            assert!(new.is_current(next, new_reveal));
            old_selected = Some(next);
            new_selected = Some(next);
            assert_popup_selection_equal(&old, &new, old_selected, new_selected);
            let old_leave = old.leave(next, false).unwrap();
            let new_key = new.profit_pointer_left().unwrap();
            let new_leave = new.leave(new_key, false).unwrap();
            assert!(old.expire(next, old_leave));
            assert!(new.expire(next, new_leave));
            assert_popup_selection_equal(&old, &new, None, None);
        }
    }
}

/// Same-bucket re-entry must fence popup dismissal; repeated interior moves must not restart entry.
#[test]
fn profit_same_bucket_reentry_survives_expire_and_repeated_moves_do_not_enter() {
    let key = PopupKey::Profit(3);
    let mut hover = PopupHover::default();
    let mut selected = None;
    hover.profit_pointer_entered();
    assert!(hover.profit_move_should_enter(3));
    model_chart_enter(&mut hover, &mut selected, key, false);
    let leaving_source = hover.profit_pointer_left().unwrap();
    let source_timer = hover.leave(leaving_source, false).unwrap();
    model_chart_enter(&mut hover, &mut selected, key, true);
    assert!(!hover.expire(key, source_timer));
    let popup_timer = hover.leave(key, true).unwrap();
    hover.profit_pointer_entered();
    assert!(hover.profit_move_should_enter(3));
    model_chart_enter(&mut hover, &mut selected, key, false);
    assert!(!hover.expire(key, popup_timer));
    assert_eq!(hover.target, Some(key));
    assert_eq!(selected, Some(key));
    let revision = hover.revision;
    for _ in 0..100 {
        assert!(!hover.profit_move_should_enter(3));
    }
    assert_eq!(hover.revision, revision);
    // A popup-first callback order still needs a chart entry even before a source-enter callback.
    hover.enter(key, true);
    assert!(hover.profit_move_should_enter(3));
    hover.enter(key, false);
    assert!(hover.profit_pointer_left().is_some());
    assert_eq!(
        hover.profit_pointer_left(),
        None,
        "gap movement must leave only once"
    );
    assert!(
        hover.profit_move_should_enter(3),
        "return across the uncovered region must cancel leave"
    );
}

/// Long identities retain their entire measured row and do not lose width to the vertical track.
#[test]
fn popup_long_rows_keep_content_and_scrollbar_width_separate() {
    let outer = popup_outer_width(460.0, 22.0, 190.0, 8.0);
    assert_eq!(outer - 22.0 - 8.0, 460.0);
    assert_eq!(popup_limits(outer, 1200.0, 800.0, 8.0).0, outer);
    assert_eq!(popup_limits(outer, 400.0, 800.0, 8.0).0, 384.0);
}

/// `summary/charts/axis.rs:distinct_core_colors` must give duplicate or missing configured colors a
/// distinct picker swatch in uid order. Dropping the taken-RGB guard makes chart lines collide,
/// while using profit order instead of uid makes the same core change color after a reload.
#[test]
fn core_colors_are_unique_and_stable_by_uid() {
    let duplicate = [17, 34, 51];
    let configured = [
        (30, Some(duplicate)),
        (10, Some(duplicate)),
        (20, None),
        (40, Some(duplicate)),
        (50, Some(duplicate)),
        (60, Some(duplicate)),
        (70, Some(duplicate)),
        (80, Some(duplicate)),
        (90, Some(duplicate)),
        (100, Some(duplicate)),
    ];
    let colors = distinct_core_colors(&configured, moon_ui::MoonPalette::LIGHT);
    let rgb: Vec<_> = colors
        .into_iter()
        .map(crate::design::hsla_to_rgb8)
        .collect();
    let unique: std::collections::HashSet<_> = rgb.iter().copied().collect();

    assert_eq!(
        unique.len(),
        configured.len(),
        "every visible core needs a distinct line color"
    );
    assert_eq!(
        rgb[1], duplicate,
        "the lowest uid keeps the user-configured color"
    );
    assert_ne!(
        rgb[2], duplicate,
        "a missing color must use an unused picker swatch"
    );

    let shuffled = [
        configured[7],
        configured[1],
        configured[9],
        configured[2],
        configured[5],
        configured[0],
        configured[8],
        configured[3],
        configured[6],
        configured[4],
    ];
    let shuffled_rgb: std::collections::HashMap<_, _> = shuffled
        .iter()
        .copied()
        .zip(distinct_core_colors(&shuffled, moon_ui::MoonPalette::LIGHT))
        .map(|((uid, _), color)| (uid, crate::design::hsla_to_rgb8(color)))
        .collect();
    for ((uid, _), color) in configured.iter().zip(rgb) {
        assert_eq!(
            shuffled_rgb.get(uid),
            Some(&color),
            "uid {uid} must keep its color across order changes"
        );
    }
}

/// Oversize content must stay inside the usable window even after font/UI or DPI changes.
/// Removing the viewport cap or subtracting only one inset lets wrapped rows leave the window.
#[test]
fn popup_limits_reserve_both_window_edges() {
    assert_eq!(popup_limits(1200.0, 860.0, 520.0, 12.0), (836.0, 496.0));
    assert_eq!(popup_limits(600.0, 430.0, 260.0, 18.0), (394.0, 224.0));
    assert_eq!(popup_limits(600.0, 20.0, 20.0, 18.0), (0.0, 0.0));
}

/// A centre bucket must not squeeze a readable popup into the remaining fraction of its plot.
/// Reintroducing the nominal plot cap loses width even when the window has enough room.
#[test]
fn popup_limits_keep_measured_width_when_window_has_room() {
    assert_eq!(popup_limits(500.0, 860.0, 520.0, 8.0), (500.0, 504.0));
    assert_eq!(popup_limits(240.0, 1240.0, 800.0, 8.0), (240.0, 784.0));
}

/// Entering the popup must invalidate the source-column timer, in either callback order.
/// Without that fence the card vanishes while the user tries to scroll its lower server names.
#[test]
fn popup_hover_survives_travel_from_column_to_card() {
    let key = PopupKey::Profit(2);
    let mut hover = PopupHover::default();
    hover.enter(key, false);
    let leaving_column = hover.leave(key, false).expect("column starts grace period");
    hover.enter(key, true);
    assert!(!hover.expire(key, leaving_column));
    assert_eq!(
        hover.leave(key, false),
        None,
        "late column leave cannot dismiss hovered card"
    );

    let leaving_card = hover.leave(key, true).expect("card starts grace period");
    assert!(
        hover.expire(key, leaving_card),
        "leaving both surfaces dismisses the card"
    );
    assert!(
        !hover.expire(key, leaving_card),
        "one timer cannot dismiss twice"
    );
}

/// Switching chart or returning across the gap must reject pending dismissal from the old owner.
#[test]
fn popup_hover_rejects_stale_leave_after_switch_or_return() {
    let first = PopupKey::Profit(2);
    let next = PopupKey::Profit(5);
    let mut hover = PopupHover::default();
    hover.enter(first, false);
    let stale = hover.leave(first, false).expect("leave timer");
    hover.enter(next, false);
    assert!(!hover.expire(first, stale));
    assert_eq!(hover.leave(first, true), None);

    let stale = hover.leave(next, false).expect("new leave timer");
    hover.enter(next, false);
    assert!(
        !hover.expire(next, stale),
        "return to same column cancels dismissal"
    );
    let current = hover.leave(next, false).expect("current leave timer");
    assert!(hover.expire(next, current));
}

/// Crossing another bucket en route to the visible popup must not replace that popup after entry.
/// Removing the reveal-generation check makes a queued neighbour dwell steal scroll ownership.
#[test]
fn popup_entry_cancels_neighbour_dwell() {
    let visible = PopupKey::Profit(10);
    let crossed = PopupKey::Profit(11);
    let mut hover = PopupHover::default();
    hover.enter(visible, false);
    hover.enter(crossed, false);
    let dwell = hover.revision;
    hover.enter(visible, true);
    assert!(!hover.is_current(crossed, dwell));
    assert_eq!(hover.leave(crossed, false), None);
    let leave = hover
        .leave(visible, true)
        .expect("leaving popup dismisses it");
    assert!(hover.expire(visible, leave));
}

/// A queued reveal cannot resurrect a bucket whose identity was invalidated by reload.
#[test]
fn popup_reload_invalidates_stale_reveal() {
    let mut hover = PopupHover::default();
    let daily = PopupKey::Profit(2);
    hover.enter(daily, false);
    let daily_dwell = hover.revision;
    hover.reset_for_reload(false);
    assert!(
        hover.is_current(daily, daily_dwell),
        "same-range daily indices remain valid"
    );
    hover.reset_for_reload(true);
    assert!(!hover.is_current(daily, daily_dwell));
}
