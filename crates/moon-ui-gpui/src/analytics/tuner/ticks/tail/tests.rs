//! The shortest tail a deal must hold.

use super::{MAX_MIN_TAIL_S, parse_s, reaches};

/// A trail reaches the minimum at it and past it; a shorter one, or no coverage, does not; 0
/// takes every covered row.
#[test]
fn a_trail_reaches_the_minimum_at_and_past_it() {
    assert!(reaches(Some((0, 60_000)), 60));
    assert!(reaches(Some((5_000, 180_000)), 60));
    assert!(!reaches(Some((60_000, 30_000)), 60));
    assert!(!reaches(None, 60));
    assert!(reaches(Some((0, 0)), 0));
}

/// A typed value is whole seconds, clamped to the store's longest margin; anything else is
/// refused.
#[test]
fn a_typed_value_is_whole_seconds_clamped_to_the_store() {
    assert_eq!(parse_s(" 90 "), Some(90));
    assert_eq!(parse_s("0"), Some(0));
    assert_eq!(parse_s("100000"), Some(MAX_MIN_TAIL_S));
    assert_eq!(parse_s("1.5"), None);
    assert_eq!(parse_s("-5"), None);
    assert_eq!(parse_s(""), None);
}
