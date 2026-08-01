//! Classified load-state presentation regression tests.

use super::{LoadState, Note};
use moon_core::db::ReadFail;

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
