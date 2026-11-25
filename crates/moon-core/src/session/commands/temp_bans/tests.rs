//! Restore a previous ban to its deadline, clearing missing or elapsed bans.

use super::*;

/// A prior state with no active ban becomes a lift.
#[test]
fn temp_undo_without_prior_clears() {
    assert_eq!(temp_restore_span(None, 10_000), None);
}

/// Restoring a ban keeps its original deadline.
#[test]
fn temp_undo_restores_prior_remaining() {
    assert_eq!(
        temp_restore_span(Some(45_000), 10_000),
        Some(Duration::from_secs(35))
    );
}

/// A deadline at or before the click becomes a lift.
#[test]
fn temp_undo_expired_prior_clears() {
    assert_eq!(temp_restore_span(Some(9_999), 10_000), None);
    assert_eq!(temp_restore_span(Some(10_000), 10_000), None);
}
