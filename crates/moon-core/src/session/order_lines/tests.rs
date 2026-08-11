use super::*;

fn order(uid: u64) -> OrderRow {
    OrderRow {
        market: "BTCUSDT".into(),
        market_display: "BTCUSDT".into(),
        coin: "BTC".into(),
        quote: "USDT".into(),
        is_short: false,
        size: 0.01,
        remaining_size: 0.01,
        sl_on: false,
        ts_on: false,
        vstop_on: false,
        sl_fixed: false,
        ts_fixed: false,
        vstop_fixed: false,
        vstop_level: 0.0,
        vstop_vol: 0.0,
        buy_price: 60_000.0,
        sell_price: 0.0,
        create_time_ms: 1_000.0,
        price: 60_000.0,
        fill_pct: 0.0,
        strat: "test".into(),
        strat_name: String::new(),
        strat_id: 0,
        status: String::new(),
        uid,
        emulator: false,
        job_is_done: false,
        pending: false,
        filled: false,
        stop_loss: None,
        trailing: None,
        take_profit: None,
        vstop: None,
        pending_cond: None,
        liq: None,
        panic_sell: false,
        is_moon_shot: false,
        corridor_price_down: 0.0,
        corridor_price_up: 0.0,
        buy_trace: None,
        sell_trace: None,
    }
}

#[test]
fn missing_order_does_not_close_without_terminal_status_or_backstop_grace() {
    let mut store = OrderLineStore::default();
    assert!(store.update(&[order(42)]));

    assert!(!store.update(&[]));

    let state = store.order_state(42).expect("retained order must stay");
    assert_eq!(state.closed_reason, None);
    assert!(state.closed_store_ms.is_none());
    assert!(state.closed_rev.is_none());
    assert!(state.active);
}

#[test]
fn terminal_status_closes_order_immediately() {
    let mut store = OrderLineStore::default();
    assert!(store.update(&[order(42)]));

    let mut done = order(42);
    done.job_is_done = true;
    assert!(store.update(&[done]));

    let state = store.order_state(42).expect("retained order must stay");
    assert_eq!(state.closed_reason, Some(OrderCloseReason::Cancel));
    assert!(state.closed_store_ms.is_some());
    assert!(state.closed_rev.is_some());
    assert!(!state.active);
}

/// `order_lines.rs:update` omitting batch removal from `open_uids` would leave a terminal order in
/// auto-fit and prevent its line from disappearing until retained-history eviction.
#[test]
fn terminal_order_leaves_auto_fit_and_revival_restores_it() {
    let mut store = OrderLineStore::default();
    assert!(store.update(&[order(42)]));
    assert_eq!(store.auto_fit_range("BTCUSDT"), Some((60_000.0, 60_000.0)));

    let mut done = order(42);
    done.job_is_done = true;
    assert!(store.update(&[done]));
    assert_eq!(store.auto_fit_range("BTCUSDT"), None);

    assert!(store.update(&[order(42)]));
    assert_eq!(store.auto_fit_range("BTCUSDT"), Some((60_000.0, 60_000.0)));
}

/// `order_lines.rs:update` marking every open UID as seen without comparing snapshot generations
/// would keep an order missing beyond the grace period visible forever.
#[test]
fn missing_order_closes_after_backstop_grace() {
    let mut store = OrderLineStore::default();
    assert!(store.update(&[order(42)]));
    store
        .orders
        .get_mut(&42)
        .expect("retained order must exist")
        .last_seen_ms -= CLOSE_GRACE_MS + 1.0;

    assert!(store.update(&[]));

    let state = store.order_state(42).expect("retained order must stay");
    assert_eq!(state.closed_reason, Some(OrderCloseReason::BackstopMissing));
    assert!(!state.active);
    assert_eq!(store.auto_fit_range("BTCUSDT"), None);
}

/// `order_lines.rs:update` counting closed UIDs as used chart slots would make labels grow with
/// retained history instead of reusing the smallest available number for a new open order.
#[test]
fn new_order_reuses_the_smallest_slot_released_by_a_terminal_order() {
    let mut store = OrderLineStore::default();
    assert!(store.update(&[order(20), order(10)]));
    assert_eq!(
        store
            .orders
            .get(&10)
            .expect("first order must exist")
            .chart_num,
        1
    );
    assert_eq!(
        store
            .orders
            .get(&20)
            .expect("second order must exist")
            .chart_num,
        2
    );

    let mut done = order(10);
    done.job_is_done = true;
    assert!(store.update(&[order(20), done]));
    assert!(store.update(&[order(20), order(30)]));

    assert_eq!(
        store
            .orders
            .get(&30)
            .expect("replacement order must exist")
            .chart_num,
        1
    );
}
