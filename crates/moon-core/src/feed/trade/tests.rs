use super::*;

/// Batch reads must isolate cores and preserve the per-kind agreement rule used by packet assembly.
#[test]
fn batch_stop_overrides_match_individual_reads() {
    let core = u64::MAX - 702;
    note_stop_override(core, 101, OrderStopKind::StopLoss, false);
    note_stop_override(core, 101, OrderStopKind::Trailing, true);
    note_stop_override(core, 102, OrderStopKind::VStop, false);
    note_stop_override(core + 1, 103, OrderStopKind::StopLoss, true);
    let batch = stop_overrides_for(core);
    assert!(!batch.contains_key(&103));
    for uid in [101, 102, 103] {
        for kind in [
            OrderStopKind::StopLoss,
            OrderStopKind::Trailing,
            OrderStopKind::VStop,
        ] {
            for wire in [false, true] {
                let from_batch = batch
                    .get(&uid)
                    .and_then(|flags| flags[stop_kind_tag(kind) as usize])
                    .filter(|on| *on == wire);
                assert_eq!(from_batch, stop_override(core, uid, kind, wire));
            }
        }
    }
    stop_overrides_map()
        .lock()
        .unwrap()
        .retain(|(server, _), _| *server != core && *server != core + 1);
}

/// Closing an order or removing its core must release both kinds of manual stop state.
#[test]
fn stop_state_ends_with_the_order_and_core() {
    let core = u64::MAX - 703;
    let other = core + 2;
    note_stop_override(core, 1, OrderStopKind::StopLoss, false);
    note_stop_override(core, 2, OrderStopKind::Trailing, false);
    note_stop_override(other, 2, OrderStopKind::Trailing, true);
    remember_stop_params(core, 1, OrderStopKind::StopLoss, false, 4.0, 0.0);
    remember_stop_params(core, 2, OrderStopKind::Trailing, false, 5.0, 0.0);

    reconcile_stop_orders(core, [(1, false), (2, true)]);

    assert!(stop_override(core, 1, OrderStopKind::StopLoss, false).is_some());
    assert!(stop_override(core, 2, OrderStopKind::Trailing, false).is_none());
    assert!(stop_memory_get(core, 1, OrderStopKind::StopLoss).is_some());
    assert!(stop_memory_get(core, 2, OrderStopKind::Trailing).is_none());
    assert!(stop_override(other, 2, OrderStopKind::Trailing, true).is_some());

    forget_stop_core(core);
    assert!(stop_overrides_for(core).is_empty());
    assert!(stop_memory_get(core, 1, OrderStopKind::StopLoss).is_none());
    assert!(stop_override(other, 2, OrderStopKind::Trailing, true).is_some());
    forget_stop_core(other);
}

/// Replacing a feed must preserve open-order stop memory; only session removal may clear it.
#[test]
fn feed_handle_drop_preserves_stop_state() {
    note_stop_override(0, 77, OrderStopKind::VStop, false);
    remember_stop_params(0, 77, OrderStopKind::VStop, false, 3.0, 1.0);
    let (_, feed) = crate::feed::trade_sound_test_feed();
    drop(feed);
    assert!(stop_override(0, 77, OrderStopKind::VStop, false).is_some());
    assert!(stop_memory_get(0, 77, OrderStopKind::VStop).is_some());
    forget_stop_core(0);
}

/// Measure a synthetic day of 100,000 edited orders shrinking to the 2,000 still open.
#[test]
#[ignore = "manual stop-state retention scale measurement"]
fn bench_stop_state_retention_day() {
    let mut samples = Vec::new();
    for _ in 0..5 {
        for core in 900_000..900_010_u64 {
            for uid in 1..=10_000_u64 {
                note_stop_override(core, uid, OrderStopKind::StopLoss, false);
                remember_stop_params(core, uid, OrderStopKind::StopLoss, false, 4.0, 0.0);
            }
        }
        let before = stop_overrides_map().lock().unwrap().len();
        let start = std::time::Instant::now();
        for core in 900_000..900_010_u64 {
            reconcile_stop_orders(core, (1..=10_000_u64).map(|uid| (uid, uid > 200)));
        }
        samples.push(start.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!(
            (before, stop_overrides_map().lock().unwrap().len()),
            (100_000, 2_000)
        );
        assert_eq!(stop_memory().lock().unwrap().len(), 2_000);
        for core in 900_000..900_010_u64 {
            forget_stop_core(core);
        }
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "[bench] stop overrides retained=100000 -> 2000 reconcile median={:.6} ms",
        samples[2]
    );
}

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
