use super::*;

fn key() -> Key {
    ("1:00000000".to_string(), "ACEUSDT".to_string())
}

const TRADE: TradeId = TradeId {
    core: 7,
    rec_id: 42,
};

#[test]
fn a_later_close_of_the_same_trade_replaces_the_first() {
    let mut closed = ClosedTrades::default();
    closed.closed(TRADE, &key(), 1_000, 2_000);
    closed.closed(TRADE, &key(), 1_000, 3_000);
    assert_eq!(closed.0.len(), 1);
    assert_eq!(closed.0[&TRADE].close_ms, 3_000);
}

#[test]
fn a_close_is_forgotten_after_the_memory_runs_out() {
    let mut closed = ClosedTrades::default();
    closed.closed(TRADE, &key(), 1_000, 2_000);
    let seen = closed.0[&TRADE].seen;
    closed.prune(seen + CLOSED_MEMORY - Duration::from_secs(1));
    assert!(closed.0.contains_key(&TRADE));
    closed.prune(seen + CLOSED_MEMORY);
    assert!(closed.0.is_empty());
}
