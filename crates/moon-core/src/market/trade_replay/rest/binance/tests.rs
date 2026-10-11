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

/// Expired Binance ranges must not close the shared gate for reachable requests on that host.
#[test]
fn expired_range_does_not_refuse_gate() {
    let error = classify(400, &serde_json::json!({"code": -4166})).unwrap_err();
    assert_eq!(error, FetchError::OutOfRetention);
    let gate = crate::market::trade_replay::gate::ReplayGate::new();
    let host = TradeRoute::BinanceUsdMAggTrades.host();
    publish_empty_notice();
    let page = crate::market::trade_replay::gate::absorb_page(&gate, host, Err(error));
    assert_eq!(page.unwrap_err(), FetchError::OutOfRetention);
    let verdict = crate::market::trade_replay::worker::TickVerdict::Abandoned(
        crate::market::trade_replay::worker::TickAbandon::OutOfRetention,
    );
    assert!(!crate::market::trade_replay::gate::settle_verdict(
        &gate,
        host,
        &verdict,
        std::time::Instant::now()
    ));
    assert!(gate.claim(host, std::time::Instant::now()).is_ok());
}

/// Clear thread-local response headers before a synthetic classifier/gate walk.
fn publish_empty_notice() {
    crate::market::trade_replay::rest::publish_exchange_notice(
        crate::market::trade_replay::rest::ExchangeNotice::default(),
    );
}
