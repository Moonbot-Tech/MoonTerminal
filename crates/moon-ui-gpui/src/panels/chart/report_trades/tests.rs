//! Regression tests for durable-history asynchronous request identity.

use super::history_result_is_current;

/// Removing the sequence check lets a slower Report scope A overwrite newer scope B on the same
/// tab; removing the target check lets core A history land after the tab moves to core B.
#[test]
fn stale_history_results_require_both_latest_sequence_and_exact_target() {
    let core_a = (7, "BTCUSDT".to_string());
    let core_b = (8, "BTCUSDT".to_string());

    assert!(!history_result_is_current(1, 2, &core_a, Some(&core_a)));
    assert!(!history_result_is_current(2, 2, &core_a, Some(&core_b)));
    assert!(history_result_is_current(2, 2, &core_b, Some(&core_b)));
}
