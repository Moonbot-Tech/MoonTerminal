use super::*;

fn order(uid: u64) -> OrderRow {
    OrderRow {
        market: "BTCUSDT".into(),
        market_display: "BTCUSDT".into(),
        is_short: false,
        size: 0.01,
        remaining_size: 0.01,
        sl_on: false,
        ts_on: false,
        sl_strat: false,
        ts_strat: false,
        vstop_strat: false,
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
