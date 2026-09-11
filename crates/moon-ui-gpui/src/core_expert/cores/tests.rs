// Explicit imports, never `use super::*`: the parent's imports include gpui-free modules only, but
// the crate convention holds for every sibling test file (CONTRIBUTING.md).
use super::{CoreRoster, RosterRow, RosterSection, anchor, keeps_changes, targets};

use moon_core::session::CoreId;

use crate::controls::row_selection::RowSelection;

/// A two-section roster: cores 1 and 2 on one venue, 3 and 4 on another, 4 without a page.
fn roster() -> CoreRoster {
    let row = |core: CoreId, has_page: bool| RosterRow {
        core,
        name: format!("core {core}").into(),
        has_page,
    };
    CoreRoster {
        sections: vec![
            RosterSection {
                label: "A".into(),
                rows: vec![row(1, true), row(2, true)],
            },
            RosterSection {
                label: "B".into(),
                rows: vec![row(3, true), row(4, false)],
            },
        ],
        order: vec![Some(1), Some(2), Some(3), Some(4)],
        key: 0,
    }
}

/// The page follows the last click; a Shift range keeps it on the far end the user pointed at.
#[test]
fn anchor_is_the_last_clicked_selected_row() {
    let roster = roster();
    let mut sel = RowSelection::default();
    sel.select_only(Some(2));
    assert_eq!(anchor(&sel, &roster), Some(2));
    sel.click(Some(4), roster.order(), true, false);
    assert_eq!(anchor(&sel, &roster), Some(4));
    assert_eq!(targets(&sel, &roster), vec![2, 3, 4]);
}

/// Ctrl-clicking the drawn core out of the selection moves the page to a core that is still going
/// to be written, in list order.
#[test]
fn anchor_falls_back_to_the_first_selected_in_list_order() {
    let roster = roster();
    let mut sel = RowSelection::default();
    sel.select_only(Some(3));
    sel.click(Some(1), roster.order(), false, true);
    assert_eq!(anchor(&sel, &roster), Some(1));
    // Ctrl-click 1 again: it leaves the selection, and the page must not stay on it.
    sel.click(Some(1), roster.order(), false, true);
    assert_eq!(anchor(&sel, &roster), Some(3));
    assert_eq!(targets(&sel, &roster), vec![3]);
}

#[test]
fn no_selection_means_no_anchor_and_no_targets() {
    let roster = roster();
    let sel = RowSelection::<CoreId>::default();
    assert_eq!(anchor(&sel, &roster), None);
    assert!(targets(&sel, &roster).is_empty());
}

/// A core that left the roster leaves the selection with it, so OK cannot address a core the
/// list no longer draws.
#[test]
fn roster_lookups_and_pruning() {
    let roster = roster();
    assert!(roster.contains(3));
    assert!(!roster.contains(9));
    assert_eq!(roster.name(2).map(|n| n.as_ref()), Some("core 2"));
    let mut sel = RowSelection::default();
    sel.select_only(Some(9));
    sel.retain_visible(roster.order());
    assert_eq!(anchor(&sel, &roster), None);
}

/// The footer counts the cores an OK reaches and the ones it has to skip separately: a selected
/// core without a page must not be reported as written.
#[test]
fn selected_pages_separates_reachable_from_skipped() {
    let roster = roster();
    let mut sel = RowSelection::default();
    sel.select_all(roster.order());
    assert_eq!(roster.selected_pages(&sel), (3, 1));
    sel.select_only(Some(4));
    assert_eq!(roster.selected_pages(&sel), (0, 1));
}

/// Changes follow the cores they were made for: kept while one of them stays selected, dropped
/// when a plain click replaces the selection with other cores.
#[test]
fn changes_survive_only_while_a_core_they_were_made_for_stays_selected() {
    assert!(
        keeps_changes(&[1], &[1, 2]),
        "Ctrl-adding a core widens the changes"
    );
    assert!(
        keeps_changes(&[1, 2], &[1]),
        "narrowing keeps them for the core left"
    );
    assert!(keeps_changes(&[1, 2], &[2, 3]));
    assert!(
        !keeps_changes(&[1], &[2]),
        "a plain click elsewhere leaves without OK"
    );
    assert!(!keeps_changes(&[1, 2], &[3]));
    assert!(!keeps_changes(&[1], &[]));
}
