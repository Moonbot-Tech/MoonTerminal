//! Unit regressions for Mini App mass-action targeting.

use super::scope_targets;

/// `mini_app.rs:scope_targets` keeps only visible cores, in visible order, once each.
///
/// Mutation: return the requested ids unfiltered or skip the dedupe. A mass
/// trading switch then commands a core the owner does not see, or the same core
/// twice. Oracle: requested [3, 99, 1, 3] against visible [1, 2, 3] is [1, 3].
#[test]
fn scope_targets_drops_unknown_and_duplicate_cores() {
    assert_eq!(scope_targets(&[3, 99, 1, 3], &[1, 2, 3]), vec![1, 3]);
    assert_eq!(scope_targets(&[99], &[1, 2, 3]), Vec::<u64>::new());
}
