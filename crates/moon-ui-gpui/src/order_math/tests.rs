//! Unit checks for the shared open-order PnL estimates.

use moon_core::feed::OrderRow;

use super::{order_pnl, position_qty};

/// Build a live long order whose numeric fields may be changed by one test.
///
/// Returns:
///     A complete row with a one-unit, filled position at the supplied entry and mark prices.
fn order(entry: f64, mark: f32) -> OrderRow {
    OrderRow {
        market: "BTCUSDT".into(),
        market_display: "BTCUSDT".into(),
        coin: "BTC".into(),
        quote: "USDT".into(),
        is_short: false,
        size: 1.0,
        remaining_size: 1.0,
        sl_on: false,
        ts_on: false,
        vstop_on: false,
        sl_fixed: false,
        ts_fixed: false,
        vstop_fixed: false,
        vstop_level: 0.0,
        vstop_vol: 0.0,
        buy_price: entry,
        sell_price: 0.0,
        create_time_ms: 0.0,
        sell_create_time_ms: 0.0,
        entry_fill_time_ms: 0.0,
        price: mark,
        fill_pct: 100.0,
        strat: "test".into(),
        strat_name: String::new(),
        strat_id: 1,
        status: String::new(),
        uid: 1,
        emulator: false,
        job_is_done: false,
        pending: false,
        filled: true,
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

/// Keep `order_math::position_qty` on its unfilled, remaining-size, and full-size branches.
///
/// Breakage: replacing the unfilled percentage calculation or either filled fallback with one size
/// field. An active sale from an already-held asset would then show the wrong PnL in both Orders and
/// the chart overlay.
#[test]
fn position_quantity_preserves_the_three_feed_lifecycle_cases() {
    let mut unfilled = order(100.0, 110.0);
    unfilled.filled = false;
    unfilled.size = 8.0;
    unfilled.fill_pct = 25.0;

    let mut partial_exit = order(100.0, 110.0);
    partial_exit.size = 8.0;
    partial_exit.remaining_size = 3.0;

    let mut held_asset_sale = order(100.0, 110.0);
    held_asset_sale.size = 8.0;
    held_asset_sale.remaining_size = 0.0;

    assert_eq!(position_qty(&unfilled), Some(2.0));
    assert_eq!(position_qty(&partial_exit), Some(3.0));
    assert_eq!(position_qty(&held_asset_sale), Some(8.0));
}

/// Keep `order_math::order_pnl` directional and unavailable without two positive prices.
///
/// Breakage: hardcoding the long direction or dropping the entry/mark gate. A profitable short
/// would become a displayed loss, or missing market data would be rendered as invented money in
/// every Orders PnL cell and the chart overlay.
#[test]
fn pnl_respects_short_direction_and_refuses_missing_prices() {
    let mut profitable_short = order(100.0, 80.0);
    profitable_short.is_short = true;
    profitable_short.size = 2.0;
    profitable_short.remaining_size = 2.0;

    let missing_entry = order(0.0, 80.0);
    let missing_mark = order(100.0, 0.0);

    assert_eq!(order_pnl(&profitable_short), Some(40.0));
    assert_eq!(order_pnl(&missing_entry), None);
    assert_eq!(order_pnl(&missing_mark), None);
}
