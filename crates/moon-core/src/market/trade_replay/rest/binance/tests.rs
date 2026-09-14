//! Regression coverage for inclusive Binance tick boundaries.

use super::*;

/// Completing on equality drops later aggregate IDs at the exit millisecond. Only crossing
/// the bound or exhausting a short page proves that all boundary siblings were fetched.
#[test]
fn full_boundary_pages_continue_until_exit_siblings_are_exhausted() {
    let row = |id, stamp| serde_json::json!({"a": id, "T": stamp, "p": "10", "q": "1", "m": false});
    let first = parse_agg_trades(&serde_json::json!([row(100, 5_000)]), 5_000, 1).unwrap();
    assert_eq!(first.next, Some(TradeCursor::FromId(101)));
    let sibling = parse_agg_trades(&serde_json::json!([row(101, 5_000)]), 5_000, 1).unwrap();
    assert_eq!(sibling.next, Some(TradeCursor::FromId(102)));
    let crossed = parse_agg_trades(&serde_json::json!([row(102, 5_001)]), 5_000, 1).unwrap();
    assert_eq!(crossed.next, None);
    let exhausted = parse_agg_trades(&serde_json::json!([row(101, 5_000)]), 5_000, 2).unwrap();
    assert_eq!(exhausted.next, None);
}
