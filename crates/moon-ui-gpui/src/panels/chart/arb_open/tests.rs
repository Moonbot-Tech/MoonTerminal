//! Which cores an arbitrage venue click may open, and which window group gets the coin.
//!
//! These cover the pure decisions only. The wiring around them — the search over the live
//! catalogs and the queued open — needs a `Backend`, which a unit test cannot build; the live click
//! is its check.

use super::{ArbAction, ArbOpen, ArbTarget, arb_action, prefer_home_group, search_scope};

/// Build one target on `core` in `group`.
fn target(core: u64, group: &str) -> ArbTarget {
    ArbTarget {
        core,
        market: "MMTUSDT".to_string(),
        group: group.to_string(),
    }
}

/// The scope keeps cores of EVERY group, not one group picked by name.
///
/// Breakage this pins: a group filter inside `search_scope` — the shape that left the click dead
/// when it searched group `""`, which no core belongs to.
#[test]
fn search_scope_spans_every_group_and_skips_the_charted_core() {
    let sessions = [(1, "default"), (2, "manual"), (3, "default"), (4, "manual")];
    let scope = search_scope(sessions, 1, |_, _| true);
    assert_eq!(
        scope,
        vec![
            (2, "manual".to_string()),
            (3, "default".to_string()),
            (4, "manual".to_string()),
        ]
    );
}

/// A core its own group's window would refuse is no target: the open would silently fail.
///
/// Breakage this pins: `search_scope` dropping the gate, or handing it anything but each core's
/// own group.
#[test]
fn search_scope_asks_the_gate_with_the_cores_own_group() {
    let sessions = [(1, "default"), (2, "manual"), (3, "manual")];
    let scope = search_scope(sessions, 1, |group, core| group == "manual" && core != 3);
    assert_eq!(scope, vec![(2, "manual".to_string())]);
}

/// The same exchange connected here and elsewhere opens here.
///
/// Breakage this pins: offering, or opening, another group's core while the clicked window has its
/// own on that exchange — the user would be moved to another window for no reason.
#[test]
fn home_group_target_wins_over_other_groups() {
    let picked = prefer_home_group(
        vec![
            target(2, "manual"),
            target(3, "default"),
            target(4, "manual"),
        ],
        Some("default"),
    );
    assert_eq!(picked, vec![target(3, "default")]);
}

/// With nothing on that exchange in this group, every other group's core stays on offer.
///
/// Breakage this pins: filtering to the home group unconditionally — the click on a venue reachable
/// only from another group would find nothing and do nothing.
#[test]
fn other_groups_are_offered_when_home_has_none() {
    let targets = vec![target(2, "manual"), target(4, "scalp")];
    assert_eq!(prefer_home_group(targets.clone(), Some("default")), targets);
}

/// A window that belongs to no group narrows nothing.
#[test]
fn no_home_group_keeps_every_target() {
    let targets = vec![target(2, "manual"), target(3, "default")];
    assert_eq!(prefer_home_group(targets.clone(), None), targets);
}

/// A target in another group opens in that group's window, whichever button was pressed.
///
/// Breakage this pins: opening another group's core with this window's authority (refused, so the
/// click does nothing), or trying a comparison across two windows.
#[test]
fn another_groups_target_opens_away_for_both_buttons() {
    for mode in [ArbOpen::Chart, ArbOpen::Compare] {
        assert_eq!(
            arb_action(mode, Some("default"), "manual"),
            ArbAction::OpenAway
        );
    }
}

/// A window with no group has no Main of its own to open on, so every target goes to its group.
///
/// Breakage this pins: treating "no home" as "here" — the chart then changes in another window
/// without raising it, and the click looks dead.
#[test]
fn a_window_without_a_group_opens_away() {
    assert_eq!(
        arb_action(ArbOpen::Chart, None, "default"),
        ArbAction::OpenAway
    );
}

/// The home group keeps each button's own meaning.
#[test]
fn home_group_target_keeps_chart_and_compare() {
    assert_eq!(
        arb_action(ArbOpen::Chart, Some("default"), "default"),
        ArbAction::OpenHere
    );
    assert_eq!(
        arb_action(ArbOpen::Compare, Some("default"), "default"),
        ArbAction::CompareHere
    );
}
