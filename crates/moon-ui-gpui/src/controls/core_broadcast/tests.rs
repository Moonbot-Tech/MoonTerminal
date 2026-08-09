use std::collections::HashSet;

use super::{apply_core_broadcast, next_core_filter};

/// `core_broadcast.rs:apply_core_broadcast` must not hand a panel a set of cores it cannot show.
///
/// Breakage: adopting the raw broadcast makes Assets prune it against its own scope on the next
/// rebuild, and a set pruned to empty means ALL cores there — one click would then widen Assets
/// while narrowing Orders to nothing, from the same value.
#[test]
fn a_panel_adopts_only_the_cores_it_can_show() {
    let mut retained = HashSet::new();
    assert!(apply_core_broadcast(
        &mut retained,
        &HashSet::from([1, 9]),
        [1, 2, 3]
    ));
    assert_eq!(
        retained,
        HashSet::from([1]),
        "the panel keeps the intersection, never a foreign id"
    );
}

/// `core_broadcast.rs:apply_core_broadcast` must leave an unrelated group alone.
///
/// Breakage: clearing on a broadcast this panel shares no core with would silently drop that
/// group's own filter, and adopting the raw ids would empty its table with nothing on screen
/// explaining why.
#[test]
fn a_broadcast_about_other_cores_is_not_this_panels_business() {
    let mut retained = HashSet::from([2]);
    assert!(
        !apply_core_broadcast(&mut retained, &HashSet::from([7]), [1, 2]),
        "a group without the clicked core must keep showing what it was showing"
    );
    assert_eq!(retained, HashSet::from([2]), "and must not be rewritten");
    assert!(!apply_core_broadcast(
        &mut retained,
        &HashSet::from([7]),
        []
    ));
}

/// `core_broadcast.rs:apply_core_broadcast` must let the release reach every panel.
///
/// Breakage: routing the empty broadcast through the intersection makes it look like "no cores in
/// common", so turning the feature off would strand every panel at its last narrowed set.
#[test]
fn the_release_reaches_panels_that_share_no_core_with_it() {
    let mut retained = HashSet::from([5]);
    assert!(apply_core_broadcast(&mut retained, &HashSet::new(), []));
    assert!(retained.is_empty());
    assert!(
        !apply_core_broadcast(&mut retained, &HashSet::new(), [1, 2]),
        "a panel already showing every core has nothing to rebuild for"
    );
}

/// `core_broadcast.rs:next_core_filter` must let the SAME gesture clear the filter it applied.
///
/// Breakage: replacing unconditionally (dropping the equality branch) leaves a user who clicked one
/// core with no way back to all cores from the monitor — the second click would be a no-op.
#[test]
fn a_plain_click_on_the_only_selected_row_clears_the_filter() {
    let current = HashSet::from([7]);
    assert!(
        next_core_filter(&current, &[7], false).is_empty(),
        "clicking the sole selected core must widen back to all cores"
    );
    assert_eq!(
        next_core_filter(&HashSet::new(), &[7], false),
        HashSet::from([7]),
        "clicking from all cores must narrow to the clicked one"
    );
    assert_eq!(
        next_core_filter(&HashSet::from([7, 9]), &[7], false),
        HashSet::from([7]),
        "a plain click replaces a multi-selection instead of toggling inside it"
    );
}

/// `core_broadcast.rs:next_core_filter` must treat a merged Exchange row as one unit.
///
/// Breakage: comparing only the first clicked core would clear the filter for an exchange row whose
/// second core is not selected, and toggling per core would half-select it under Ctrl.
#[test]
fn a_merged_row_selects_and_clears_as_one() {
    let current = HashSet::from([1, 2]);
    assert!(
        next_core_filter(&current, &[1, 2], false).is_empty(),
        "an exchange row that IS the selection clears it"
    );
    assert_eq!(
        next_core_filter(&HashSet::from([1]), &[1, 2], false),
        HashSet::from([1, 2]),
        "a partially selected exchange row becomes the whole selection"
    );
    assert_eq!(
        next_core_filter(&HashSet::from([1, 2, 5]), &[1, 2], true),
        HashSet::from([5]),
        "Ctrl removes a fully selected exchange row without touching the others"
    );
}

/// `core_broadcast.rs:next_core_filter` must accumulate under the secondary modifier.
///
/// Breakage: routing Ctrl through the replace branch turns multi-select into single-select, which
/// is exactly the gesture the monitor's setting promises.
#[test]
fn ctrl_adds_and_removes_without_replacing() {
    let mut current = HashSet::new();
    current = next_core_filter(&current, &[3], true);
    current = next_core_filter(&current, &[8], true);
    assert_eq!(current, HashSet::from([3, 8]), "Ctrl accumulates cores");

    current = next_core_filter(&current, &[3], true);
    assert_eq!(current, HashSet::from([8]), "Ctrl removes a selected core");

    current = next_core_filter(&current, &[8], true);
    assert!(
        current.is_empty(),
        "removing the last core leaves the empty set, which every consumer reads as all cores"
    );
}

/// `core_broadcast.rs:next_core_filter` must ignore a row that carries no cores.
///
/// Breakage: falling through with an empty clicked set makes a plain click on an empty row clear a
/// filter the user did not ask to clear, and a Ctrl click a silent no-op that still repaints.
#[test]
fn a_row_without_cores_cannot_change_the_selection() {
    let current = HashSet::from([4]);
    assert_eq!(next_core_filter(&current, &[], false), current);
    assert_eq!(next_core_filter(&current, &[], true), current);
}
