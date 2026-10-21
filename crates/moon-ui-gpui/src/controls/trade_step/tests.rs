//! Unit tests for the session step a pick stores.

use super::session_step;

#[test]
fn picking_the_stored_step_returns_the_tab_to_following_it() {
    assert_eq!(session_step(2, 2), None);
    assert_eq!(session_step(0, 0), None);
}

#[test]
fn picking_another_step_overrides_for_the_session() {
    assert_eq!(session_step(4, 2), Some(4));
    assert_eq!(session_step(0, 10), Some(0));
}
