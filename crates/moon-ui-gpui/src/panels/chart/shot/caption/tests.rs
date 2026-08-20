//! Regression coverage for the chart-shot capture gate.

use super::{WaitStep, wait_step};

/// `caption.rs:wait_step` must keep `(false, 0)` as `GiveUp`, rather than capturing when the
/// renderer has not proved the exchange caption reached the desktop; otherwise the clipboard can
/// receive a chart whose corner still exposes the user's account name.
#[test]
fn undrawn_caption_is_never_captured_after_the_wait_budget_expires() {
    assert_eq!(wait_step(false, 0), WaitStep::GiveUp);
    assert_eq!(wait_step(false, 1), WaitStep::Wait);
    assert_eq!(wait_step(true, 0), WaitStep::Capture);
}
