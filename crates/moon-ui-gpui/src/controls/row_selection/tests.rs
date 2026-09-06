//! Contract tests for the shared Core Status row-selection state.

use super::RowSelection;

type Key = u64;

/// Return selected keys in a deterministic order so membership, rather than `HashSet` iteration,
/// is the test oracle.
fn selected_keys(selection: &RowSelection<Key>) -> Vec<Key> {
    let mut keys = selection.iter().copied().collect::<Vec<_>>();
    keys.sort_unstable();
    keys
}

/// `controls/row_selection.rs:RowSelection::click` must leave state unchanged for `None`.
/// Removing that guard would let an exchange-heading click retarget a later Shift selection.
#[test]
fn none_click_is_a_full_noop() {
    let order = [Some(1), None, Some(2), Some(3), Some(4)];
    let mut selection = RowSelection::default();
    selection.click(Some(2), &order, false, false);

    selection.click(None, &order, true, true);

    assert_eq!(selected_keys(&selection), vec![2]);
    assert_eq!(selection.current(), Some(2));
    selection.click(Some(4), &order, true, false);
    assert_eq!(
        selected_keys(&selection),
        vec![2, 3, 4],
        "the ignored heading must not replace the Shift anchor"
    );
}

/// `controls/row_selection.rs:RowSelection::click` must make Shift win over Ctrl and retain its
/// original anchor. Letting Shift fall through to Ctrl would make the visibly highlighted update
/// set disagree with the menu count and the cores that receive a build.
#[test]
fn shift_selects_the_exact_visible_span_and_keeps_its_anchor() {
    let order = [Some(1), None, Some(2), Some(3), Some(4)];
    let mut selection = RowSelection::default();
    selection.click(Some(1), &order, false, false);
    selection.click(Some(4), &order, false, true);

    selection.click(Some(2), &order, true, true);

    assert_eq!(selected_keys(&selection), vec![2, 3, 4]);
    assert_eq!(selection.current(), Some(2));
    selection.click(Some(1), &order, true, false);
    assert_eq!(
        selected_keys(&selection),
        vec![1, 2, 3, 4],
        "the second range must still start at the original anchor"
    );

    let mut without_anchor = RowSelection::default();
    without_anchor.click(Some(3), &order, true, false);
    assert_eq!(selected_keys(&without_anchor), vec![3]);
    without_anchor.click(Some(1), &order, true, false);
    assert_eq!(selected_keys(&without_anchor), vec![1, 2, 3]);
}

/// `controls/row_selection.rs:RowSelection::click` must clear a sole plain-click selection but
/// preserve its anchor, while Ctrl deselection makes `current()` absent. Replacing either branch
/// with an unconditional insert leaves invisible or stale cores in the next bulk update scope.
#[test]
fn plain_and_secondary_clicks_preserve_the_selection_contract() {
    let order = [Some(1), None, Some(2), Some(3), Some(4)];
    let mut selection = RowSelection::default();
    selection.click(Some(2), &order, false, false);
    selection.click(Some(2), &order, false, false);

    assert_eq!(selection.len(), 0);
    assert_eq!(selection.current(), None);
    selection.click(Some(4), &order, true, false);
    assert_eq!(
        selected_keys(&selection),
        vec![2, 3, 4],
        "the clearing plain click deliberately leaves a Shift anchor"
    );

    selection.click(Some(3), &order, false, false);
    selection.click(Some(3), &order, false, true);
    assert_eq!(selection.len(), 0);
    assert_eq!(
        selection.current(),
        None,
        "a Ctrl-deselected last click cannot remain the current core"
    );
    selection.click(Some(4), &order, true, false);
    assert_eq!(selected_keys(&selection), vec![3, 4]);

    selection.select_only(Some(1));
    selection.select_only(None);
    assert_eq!(selected_keys(&selection), vec![1]);
    assert_eq!(selection.current(), Some(1));
}

/// `controls/row_selection.rs:RowSelection::{select_all,retain_visible,clear}` must skip heading
/// rows, retain the pre-existing anchor, and remove vanished identities. Keeping a hidden core
/// selected would silently send its next update even though the operator cannot see it.
#[test]
fn bulk_and_visibility_operations_keep_only_visible_core_identities() {
    let order = [Some(1), None, Some(2), Some(3), Some(4)];
    let mut selection = RowSelection::default();
    selection.click(Some(2), &order, false, false);
    selection.select_all(&order);

    assert_eq!(selected_keys(&selection), vec![1, 2, 3, 4]);
    selection.click(Some(3), &order, true, false);
    assert_eq!(
        selected_keys(&selection),
        vec![2, 3],
        "select_all must not synthesize a new Shift anchor"
    );

    selection.click(Some(1), &order, false, true);
    selection.click(Some(4), &order, false, true);
    selection.retain_visible(&[Some(1), None]);
    assert_eq!(selected_keys(&selection), vec![1]);
    assert_eq!(selection.current(), None);
    selection.click(Some(3), &order, true, false);
    assert_eq!(selected_keys(&selection), vec![3]);

    selection.clear();
    assert_eq!(selection.len(), 0);
    assert_eq!(selection.current(), None);
    selection.click(Some(4), &order, true, false);
    assert_eq!(selected_keys(&selection), vec![4]);
}
