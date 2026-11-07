//! The version selection across a reload of the list.

use super::selection_survives;

/// Regression target: keeping a stamp a merge moved. The pane would stay read-only on a version
/// that no longer exists under that stamp, with no row highlighted.
#[test]
fn a_vanished_stamp_closes_the_selection() {
    assert!(selection_survives(200, [100, 200, 300].into_iter()));
    assert!(!selection_survives(200, [100, 150, 300].into_iter()));
}

/// An empty list is also what a failed read returns; a snapshot being read must not turn into the
/// live, editable strategy because of it.
#[test]
fn an_empty_list_keeps_the_selection() {
    assert!(selection_survives(200, std::iter::empty()));
}
