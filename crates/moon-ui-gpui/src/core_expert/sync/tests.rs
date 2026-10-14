//! The exact-core reveal leaves another core's staged edits alone.

use super::{ExpertReveal, expert_reveal_decision};

/// A clean window may show the coin. A window already on that core may too, even with edits.
/// Staged edits for a different core stay put.
#[test]
fn expert_reveal_decision_keeps_a_dirty_other_core() {
    assert_eq!(expert_reveal_decision(true, false), ExpertReveal::Show);
    assert_eq!(expert_reveal_decision(true, true), ExpertReveal::Show);
    assert_eq!(expert_reveal_decision(false, true), ExpertReveal::Show);
    assert_eq!(
        expert_reveal_decision(false, false),
        ExpertReveal::DirtyOtherCore
    );
}
