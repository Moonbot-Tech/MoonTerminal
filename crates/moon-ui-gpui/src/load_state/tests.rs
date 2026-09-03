//! Classified load-state presentation regression tests.

use super::{LoadState, Note};
use moon_core::db::{FailKind, ReadFail};
use std::sync::Arc;

/// Construct a classified read failure without coupling tests to a rendered error message.
fn failure(kind: FailKind) -> ReadFail {
    ReadFail::Failed {
        kind,
        msg: Arc::from("test failure"),
    }
}

/// `load_state.rs:LoadState::apply_or_keep` must retain only a settled stale snapshot when a
/// report catch-up gets Busy. Keeping a NotReady result would show numbers from an unavailable
/// replica, while dropping a valid stale snapshot makes the Analytics surface flash to an error.
#[test]
fn catch_up_failure_keeps_only_a_real_stale_snapshot() {
    let mut stale = LoadState::Ready(Arc::new(vec![42]));
    stale.begin();
    stale.apply_or_keep(Err(failure(FailKind::Busy)), true);
    assert!(
        matches!(&stale, LoadState::Ready(_)),
        "a Busy revalidation restores the settled stale snapshot"
    );

    let mut no_stale = LoadState::<Vec<i32>>::default();
    no_stale.apply_or_keep(Err(failure(FailKind::Busy)), true);
    assert!(
        matches!(&no_stale, LoadState::Failed(_)),
        "an initial read failure has no snapshot to preserve"
    );

    let mut unavailable = LoadState::Ready(Arc::new(vec![7]));
    unavailable.begin();
    unavailable.apply_or_keep(Err(ReadFail::NotReady), true);
    assert!(
        matches!(&unavailable, LoadState::NotReady),
        "NotReady is a completed unavailable-replica answer, never stale data"
    );
}

/// `load_state.rs:LoadState::apply_or_keep` must publish a failure when preservation is disabled.
/// Keeping the old snapshot after a user scope change would put the prior scope's numbers under
/// the new label, which is worse than a visible classified read failure.
#[test]
fn scope_changes_do_not_keep_the_previous_snapshot_after_a_failure() {
    let mut state = LoadState::Ready(Arc::new(vec![42]));
    state.begin();
    state.apply_or_keep(Err(failure(FailKind::Busy)), false);
    assert!(
        matches!(&state, LoadState::Failed(_)),
        "a non-preserving scope change must surface the completed failure"
    );

    state.apply_or_keep(Ok(vec![9]), true);
    assert!(
        matches!(&state, LoadState::Ready(_)),
        "a successful replacement still settles normally"
    );
}

/// Incomparable quote scope is guidance, not a reports-database failure.
///
/// Mapping `ReadFail::IncomparableQuote` through the generic failure branch in
/// `load_state.rs:LoadState::view` makes a healthy mixed-currency tuner scope tell the user that
/// the database could not be read.
#[test]
fn incomparable_quote_has_a_non_database_note() {
    let mut state = LoadState::<Vec<()>>::default();
    state.apply(Err(ReadFail::IncomparableQuote));

    assert!(matches!(
        state.view(Vec::is_empty),
        Err(Note::IncomparableQuote)
    ));
}
