// Explicit imports, never `use super::*` (CONTRIBUTING.md).
use super::{MixedScope, accent, cached, install, take};

/// A control is mixed when a field it writes is among the ones needing attention; a dead control
/// (writes nothing) and an unprobed id never are.
#[test]
fn mixed_means_a_written_field_needs_attention() {
    let mut scope = MixedScope::with_probes(&[("tp", &[3]), ("radio", &[5, 7]), ("dead", &[])]);
    scope.set_context([3, 7].into_iter(), 0);
    assert!(scope.cached("tp"));
    assert!(scope.cached("radio"));
    assert!(!scope.cached("dead"));
    assert!(!scope.cached("never-probed"));
    assert_eq!(scope.differing(), &[3, 7]);

    // The attention list no longer names the field: the control shows a value again.
    scope.set_context([7].into_iter(), 0);
    assert!(!scope.cached("tp"));
    assert!(scope.cached("radio"), "its other field still differs");

    // Nothing differs: nothing is mixed, whatever was probed.
    scope.set_context(std::iter::empty(), 0);
    assert!(!scope.cached("tp"));
}

/// Without a seeded page there is nothing to probe against, and the probe is not run.
#[test]
fn unseeded_scope_never_probes() {
    let mut scope = MixedScope::default();
    scope.set_context([1].into_iter(), 0);
    let ran = std::cell::Cell::new(false);
    assert!(!scope.is_mixed("x", &|_| ran.set(true)));
    assert!(!ran.get());
}

/// Outside a render — no scope installed — every question answers "not mixed", and the scope
/// round-trips through install/take with its context.
#[test]
fn no_scope_installed_reads_as_not_mixed() {
    // Whatever another test left behind on this thread is taken first.
    let _ = take();
    assert!(!cached("tp"));
    assert_eq!(accent(), None);
    let mut scope = MixedScope::with_probes(&[("tp", &[3])]);
    scope.set_context([3].into_iter(), 0xff00ff);
    install(scope);
    assert!(cached("tp"));
    assert_eq!(accent(), Some(0xff00ff));
    let back = take();
    assert!(back.cached("tp"));
    assert!(!cached("tp"));
}
