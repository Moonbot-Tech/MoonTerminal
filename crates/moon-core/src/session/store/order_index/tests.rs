//! Parity checks for the order-batch indexes consumed by Mini App reads.

use super::*;

/// One neutral synthetic order with only the indexed fields varied by the test.
fn order(uid: u64) -> OrderRow {
    OrderRow {
        market: "XALPHAUSDT".into(),
        market_display: "XALPHAUSDT".into(),
        coin: "XALPHA".into(),
        quote: "USDT".into(),
        is_short: false,
        size: 1.0,
        remaining_size: 0.0,
        sl_on: false,
        ts_on: false,
        vstop_on: false,
        sl_fixed: false,
        ts_fixed: false,
        vstop_fixed: false,
        vstop_level: 0.0,
        vstop_vol: 0.0,
        buy_price: 100.0,
        sell_price: 0.0,
        create_time_ms: 0.0,
        sell_create_time_ms: 0.0,
        entry_fill_time_ms: 0.0,
        price: 100.0,
        fill_pct: 0.0,
        strat: "UNIT_01".into(),
        strat_name: "UNIT_01".into(),
        strat_id: 7,
        status: String::new(),
        uid,
        emulator: false,
        job_is_done: false,
        pending: true,
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

/// Filtering or reordering index members would drop live coins or change their displayed order.
#[test]
fn strategy_membership_matches_the_frozen_order_scan() {
    let mut rows = vec![order(1), order(2), order(3), order(4)];
    rows[1].job_is_done = true;
    rows[1].market = "XBETAUSDT".into();
    rows[1].quote = String::new();
    rows[2].strat_id = 8;
    rows[3].market = "XBETAUSDT".into();
    rows[3].quote = String::new();
    let index = OrderIndex::new(&rows);
    let actual: HashMap<u64, Vec<usize>> = index
        .strategies()
        .map(|(strategy, rows)| (strategy, rows.to_vec()))
        .collect();
    let expected: HashMap<u64, Vec<usize>> = [7, 8]
        .into_iter()
        .map(|strategy| {
            (
                strategy,
                rows.iter()
                    .enumerate()
                    .filter(|(_, order)| !order.job_is_done && order.strat_id == strategy)
                    .map(|(position, _)| position)
                    .collect(),
            )
        })
        .collect();
    assert_eq!(actual, expected);
    let pairs: Vec<_> = index.quote_markets().collect();
    assert_eq!(pairs, [("", "XBETAUSDT"), ("USDT", "XALPHAUSDT")]);
}
