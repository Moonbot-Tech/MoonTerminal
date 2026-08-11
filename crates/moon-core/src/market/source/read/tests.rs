use super::*;

#[test]
fn fresh_but_shallow_deep_rows_still_request_history() {
    assert_eq!(
        deep_history_request_reason(Some((2, 90_000, 120_000)), 0, 120_000, 60_000),
        Some(DeepHistoryRequestReason::Shallow)
    );
}

#[test]
fn fresh_rows_that_do_not_cover_the_window_request_history() {
    assert_eq!(
        deep_history_request_reason(Some((60, 600_000, 900_000)), 0, 900_000, 60_000),
        Some(DeepHistoryRequestReason::DoesNotCoverWindow)
    );
}

#[test]
fn deep_rows_covering_the_window_do_not_request_history() {
    assert_eq!(
        deep_history_request_reason(Some((60, 0, 900_000)), 0, 900_000, 60_000),
        None
    );
}
