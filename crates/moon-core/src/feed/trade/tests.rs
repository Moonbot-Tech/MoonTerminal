use super::*;

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
