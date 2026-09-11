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

/// Regression target: the wire's `planned_sell_price` is an ABSOLUTE price, and every value the
/// terminal could derive for a pending comes from the TRIGGER — the core moves the real entry off it
/// by its own pending spread (`SharedConfig::pending_orders_spread`, shipped at 0.5%). Growing this
/// builder a sell target therefore puts a silently wrong take profit on money.
#[test]
fn pending_order_params_carry_no_sell_target() {
    let params = pending_order_params("BTCUSDT".to_string(), false, 100_000.0, 0.001, None, true);

    assert_eq!(params.planned_sell_price, 0.0);
    // The price rides as the CONDITION field, never as an order price.
    assert_eq!(params.trigger_price, 100_000.0);
}

/// Regression target: `short` is the POSITION side on this path as it is on `new_order`, and a
/// pending whose side flipped opens the opposite position when the trigger fires.
#[test]
fn pending_order_params_map_the_position_side() {
    let long = pending_order_params("BTCUSDT".to_string(), false, 100_000.0, 0.001, None, false);
    let short = pending_order_params("BTCUSDT".to_string(), true, 100_000.0, 0.001, None, false);

    assert_eq!(long.side, OrderSide::Long);
    assert_eq!(short.side, OrderSide::Short);
}

/// Regression target: a pending with NO strategy is bare — the core does not substitute its own
/// manual strategy into one, unlike a new order — so passing `Some(id)` through is the only way a
/// terminal-selected manual strategy ever reaches it.
#[test]
fn pending_order_params_carry_an_explicit_strategy_only() {
    let bare = pending_order_params("BTCUSDT".to_string(), false, 100_000.0, 0.001, None, false);
    let named = pending_order_params(
        "BTCUSDT".to_string(),
        false,
        100_000.0,
        0.001,
        Some(77),
        false,
    );

    assert_eq!(bare.strategy_id, None);
    assert_eq!(named.strategy_id, Some(77));
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

/// The order size IS the coin quantity; only the price is derived.
///
/// Regression target, measured on Bitget 2026-09-03: sizing the order in the account's balance
/// currency instead sent `size=79.679` for a 198.01 AERO holding, and the core sold exactly
/// 79.67 AERO — the number taken as coins and snapped to the lot step. The same mistake on
/// MANTRA offered $0.37 of a $103 holding and was rejected for the venue's 1 USDT minimum.
#[test]
fn a_spot_market_sale_sends_the_coin_quantity_as_the_size() {
    let (limit, size) = market_sell_terms(198.01, 0.5031).expect("a priced holding has terms");

    assert_eq!(
        size, 198.01,
        "the whole held quantity must ride as the size"
    );
    // Ported from the core's own sale, which priced at exactly `last * 0.8` and filled at market.
    assert!((limit - 0.40248).abs() < 1e-9, "limit price was {limit}");
    assert!(limit < 0.5031, "a sell must price THROUGH the book to fill");
}

/// Inputs that cannot produce an order size send nothing, rather than a zero the core reinterprets.
///
/// Mutation: restore the old `price=0` call. A zero price is precisely the input whose fallback
/// inside the core produced the runaway quantity, so it must not reach the wire.
#[test]
fn unpriced_or_empty_holdings_yield_no_sell_terms() {
    assert!(market_sell_terms(0.005, 0.0).is_none());
    assert!(market_sell_terms(0.005, -1.0).is_none());
    assert!(market_sell_terms(0.005, f64::NAN).is_none());
    assert!(market_sell_terms(0.005, f64::INFINITY).is_none());
    assert!(market_sell_terms(0.0, 78_528.68).is_none());
    assert!(market_sell_terms(-0.005, 78_528.68).is_none());
    assert!(market_sell_terms(f64::NAN, 78_528.68).is_none());
    // A finite price cannot overflow the limit, which only ever scales it DOWN, so an absurd but
    // finite pair is not a refusal — the venue's own filters are what reject it.
    assert!(market_sell_terms(f64::MAX, f64::MAX).is_some());
}
