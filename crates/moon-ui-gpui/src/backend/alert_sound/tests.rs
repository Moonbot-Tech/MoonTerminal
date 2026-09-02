//! The edge rule behind the price-approach alert: announce a leg that has just come inside its
//! band, and only then.

use std::collections::HashSet;

use super::{AlertLeg, leg_to_announce};

fn set(legs: &[(u64, AlertLeg)]) -> HashSet<(u64, AlertLeg)> {
    legs.iter().copied().collect()
}

/// Both alerts armed, which is the case every test but the last one is about.
fn both(_: AlertLeg) -> bool {
    true
}

/// A leg already inside its band on the previous pass stays silent.
///
/// Plausible breakage: announcing the LEVEL rather than the edge re-fires for as long as the price
/// stays inside the band — several times a second on a moving market, which is a siren rather than
/// an alert, and no test of the distance arithmetic would notice.
#[test]
fn a_leg_that_was_already_near_stays_silent() {
    let inside = set(&[(7, AlertLeg::Exit)]);
    assert_eq!(leg_to_announce(&inside, &inside, both), None);
    // It leaves the band and comes back: that IS a new crossing and announces again.
    assert_eq!(
        leg_to_announce(&inside, &set(&[]), both),
        Some(AlertLeg::Exit)
    );
}

/// Leaving the band announces nothing; only an inward crossing does.
#[test]
fn leaving_the_band_is_not_an_announcement() {
    assert_eq!(
        leg_to_announce(&set(&[]), &set(&[(7, AlertLeg::Exit)]), both),
        None
    );
}

/// The exit wins when both legs cross in the same batch, and exactly one leg is announced.
///
/// Plausible breakage: playing both makes the player cut the first sound off mid-way, so the user
/// hears one clipped noise and cannot tell which alert fired.
#[test]
fn the_exit_is_announced_first_when_both_cross() {
    let near = set(&[(7, AlertLeg::Entry), (9, AlertLeg::Exit)]);
    assert_eq!(
        leg_to_announce(&near, &set(&[]), both),
        Some(AlertLeg::Exit)
    );
}

/// An UNARMED exit does not take the pass from an armed entry that crossed in the same batch.
///
/// Plausible breakage: filtering the winner instead of the candidates makes the exit — which wins
/// the tie — swallow the pass and answer `None`, so the entry alert stays silent for as long as any
/// exit keeps crossing. Both legs are tracked whichever alert is armed, which is what puts an
/// unarmed leg in front of this filter at all.
#[test]
fn an_unarmed_leg_does_not_swallow_the_pass() {
    let near = set(&[(7, AlertLeg::Entry), (9, AlertLeg::Exit)]);
    let entry_only = |leg: AlertLeg| leg == AlertLeg::Entry;
    assert_eq!(
        leg_to_announce(&near, &set(&[]), entry_only),
        Some(AlertLeg::Entry)
    );
    // And with nothing armed at all, nothing is announced.
    assert_eq!(leg_to_announce(&near, &set(&[]), |_| false), None);
}

/// The retained set is keyed by ORDER AND LEG, so one leg being inside its band cannot suppress
/// the other's announcement.
///
/// `filled` makes the two exclusive for a live order today — an order is either waiting at its
/// entry or holding a position — so this states the keying rather than a state the scan reaches.
/// Plausible breakage: keying by order id alone silently drops one of the two the day an order
/// carries both.
#[test]
fn the_two_legs_of_one_order_do_not_shadow_each_other() {
    let was = set(&[(7, AlertLeg::Exit)]);
    let near = set(&[(7, AlertLeg::Exit), (7, AlertLeg::Entry)]);
    assert_eq!(leg_to_announce(&near, &was, both), Some(AlertLeg::Entry));
}

/// A different order crossing while another is already inside its band is still announced.
#[test]
fn another_order_crossing_is_its_own_announcement() {
    let was = set(&[(1, AlertLeg::Exit)]);
    let near = set(&[(1, AlertLeg::Exit), (2, AlertLeg::Exit)]);
    assert_eq!(leg_to_announce(&near, &was, both), Some(AlertLeg::Exit));
}
