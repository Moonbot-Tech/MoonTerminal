use super::*;

#[test]
/// Regression target: deleting `with_market_stop` from `trade::new_order_params` makes a visible
/// Stop Market selection place a stop-limit order instead.
fn new_order_params_carry_stop_market() {
    let params = new_order_params(
        "BTCUSDT".to_string(),
        false,
        100_000.0,
        0.001,
        None,
        true,
        0.0,
    );

    assert!(params.use_market_stop);
}

/// Regression target: the visible take profit reaches a manual order ONLY as this field — Moonbot
/// stores it on the order rather than in the strategy — so dropping it from `new_order_params`
/// silently places every manual order with no sell target of its own.
#[test]
fn new_order_params_carry_a_positive_planned_sell_only() {
    let with_target = new_order_params(
        "BTCUSDT".to_string(),
        false,
        100_000.0,
        0.001,
        None,
        false,
        101_000.0,
    );
    assert_eq!(with_target.planned_sell_price, 101_000.0);

    // Zero is the wire's "no target": the core then applies its own settings or the order's
    // strategy, and sending it as a price would ask for a sell at zero.
    let without = new_order_params(
        "BTCUSDT".to_string(),
        false,
        100_000.0,
        0.001,
        None,
        false,
        0.0,
    );
    assert_eq!(without.planned_sell_price, 0.0);
}

/// A `(market_name, uid, has_live_sell_leg)` candidate for the resolver.
fn candidate(market: &'static str, uid: u64, sell: bool) -> (&'static str, u64, bool) {
    (market, uid, sell)
}

/// Exactly one order on the market with a live sell leg → its uid.
#[test]
fn resolves_sole_active_sell_order() {
    let orders = [
        candidate("BTCUSDT", 10, true),
        candidate("ETHUSDT", 11, true),
        candidate("BTCUSDT", 12, false),
    ];
    assert_eq!(
        resolve_market_split_target(orders.into_iter(), "BTCUSDT"),
        SplitTarget::One(10)
    );
}

/// Two live sell orders on the market → ambiguous. A future edit that returned
/// the FIRST match instead of enforcing uniqueness would split the wrong order
/// (real money) and reddens here.
#[test]
fn refuses_two_active_sell_orders() {
    let orders = [
        candidate("BTCUSDT", 10, true),
        candidate("BTCUSDT", 12, true),
    ];
    assert_eq!(
        resolve_market_split_target(orders.into_iter(), "BTCUSDT"),
        SplitTarget::Ambiguous
    );
}

/// No live sell leg on the market → ambiguous (nothing to split). An edit that
/// dropped the sell-leg gate would pick a buy-side order and reddens here.
#[test]
fn refuses_when_no_active_sell_order() {
    let orders = [
        candidate("BTCUSDT", 10, false),
        candidate("ETHUSDT", 11, true),
    ];
    assert_eq!(
        resolve_market_split_target(orders.into_iter(), "BTCUSDT"),
        SplitTarget::Ambiguous
    );
}
