//! Regression tests for Orders header and legacy sorting.

use std::collections::HashMap;

use moon_core::feed::OrderRow;

use super::sort_entries;
use crate::panels::orders::{OrdCol, OrderEntry, OrdersViewState};

/// Build one sortable order with only the fields relevant to these comparator tests varied later.
fn entry(core: u64, uid: u64, coin: &str, size: f64) -> OrderEntry {
    OrderEntry {
        core,
        core_name: format!("Core {core}"),
        row: OrderRow {
            market: format!("{coin}USDT"),
            market_display: format!("{coin}USDT"),
            coin: coin.to_string(),
            quote: "USDT".to_string(),
            is_short: false,
            size,
            remaining_size: size,
            sl_on: false,
            ts_on: false,
            vstop_on: false,
            sl_fixed: false,
            ts_fixed: false,
            vstop_fixed: false,
            vstop_level: 0.0,
            vstop_vol: 0.0,
            buy_price: 10.0,
            sell_price: 12.0,
            create_time_ms: uid as f64,
            sell_create_time_ms: 0.0,
            entry_fill_time_ms: 0.0,
            price: 11.0,
            fill_pct: 100.0,
            strat: "EMA".to_string(),
            strat_name: String::new(),
            strat_id: 1,
            status: "BuyDone".to_string(),
            uid,
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
        },
    }
}

/// `orders/sort.rs:sort_entries` must apply both directions to the selected numeric column.
///
/// Mutation: ignore `ascending` for `OrdCol::Size`. The second assertion reddens and proves a
/// repeated Size-header click would leave the row order pointing opposite to its arrow.
#[test]
fn numeric_header_sort_follows_both_arrow_directions() {
    let overlays = HashMap::new();
    let mut view = OrdersViewState {
        header_sort: Some((OrdCol::Size, true)),
        ..OrdersViewState::default()
    };
    let mut rows = [entry(1, 1, "AAA", 2.0), entry(1, 2, "BBB", 1.0)];
    sort_entries(&mut rows, &view, &overlays);
    assert_eq!(
        rows.iter().map(|row| row.row.size).collect::<Vec<_>>(),
        vec![1.0, 2.0]
    );

    view.header_sort = Some((OrdCol::Size, false));
    sort_entries(&mut rows, &view, &overlays);
    assert_eq!(
        rows.iter().map(|row| row.row.size).collect::<Vec<_>>(),
        vec![2.0, 1.0]
    );
}

/// `orders/sort.rs:sort_entries` must compare the Token text shown in the cells.
///
/// Mutation: compare `market` instead of `coin`. Exchange-specific market syntax could reorder a
/// different token sequence than the visible column, and the asserted display-token order reddens.
#[test]
fn text_header_sort_uses_the_displayed_token() {
    let overlays = HashMap::new();
    let view = OrdersViewState {
        header_sort: Some((OrdCol::Token, true)),
        ..OrdersViewState::default()
    };
    let mut first = entry(1, 1, "ZED", 1.0);
    first.row.market = "@1".to_string();
    let mut second = entry(1, 2, "ALPHA", 1.0);
    second.row.market = "ZZZUSDT".to_string();
    let mut rows = [first, second];

    sort_entries(&mut rows, &view, &overlays);

    assert_eq!(
        rows.iter()
            .map(|row| row.row.coin.as_str())
            .collect::<Vec<_>>(),
        vec!["ALPHA", "ZED"]
    );
}

/// `orders/sort.rs:sort_entries` must keep dash-valued strategy names after real names both ways.
///
/// Mutation: reverse the whole `Option` comparison for descending. The unnamed row would rise to
/// the top under the down arrow, and the second assertion reddens.
#[test]
fn missing_header_values_stay_last_in_both_directions() {
    let overlays = HashMap::new();
    let mut named = entry(1, 1, "AAA", 1.0);
    named.row.strat_name = "Trend".to_string();
    let unnamed = entry(1, 2, "BBB", 1.0);
    let mut rows = [unnamed, named];
    for ascending in [true, false] {
        let view = OrdersViewState {
            header_sort: Some((OrdCol::StratName, ascending)),
            ..OrdersViewState::default()
        };
        sort_entries(&mut rows, &view, &overlays);
        assert_eq!(rows[0].row.strat_name, "Trend");
        assert!(rows[1].row.strat_name.is_empty());
    }
}

/// `orders/sort.rs:sort_entries` must use the optimistic stop value the cell currently displays.
///
/// Mutation: compare only `OrderRow::sl_on`. The overlaid OFF row remains first instead of moving
/// after the baked OFF row, so the asserted uid order reddens while the table visibly says OFF/OFF.
#[test]
fn stop_sort_uses_the_optimistic_display_value() {
    let mut baked_on = entry(1, 1, "AAA", 1.0);
    baked_on.row.sl_on = true;
    let baked_off = entry(1, 2, "BBB", 1.0);
    let mut overlays = HashMap::new();
    overlays.insert((1, 1, 0), (false, std::time::Instant::now()));
    let view = OrdersViewState {
        header_sort: Some((OrdCol::Sl, true)),
        ..OrdersViewState::default()
    };
    let mut rows = [baked_on, baked_off];

    sort_entries(&mut rows, &view, &overlays);

    assert_eq!(
        rows.iter().map(|row| row.row.uid).collect::<Vec<_>>(),
        vec![1, 2]
    );
}

/// `orders/sort.rs:sort_entries` must break equal visible keys by stable identity, not input order.
///
/// Mutation: remove the `(core, uid)` fallback. Reversing the input below would survive the sort,
/// and rows with equal PnL would reshuffle whenever feeds rebuild the same visible table.
#[test]
fn equal_header_values_use_a_direction_independent_identity_tie() {
    let overlays = HashMap::new();
    let view = OrdersViewState {
        header_sort: Some((OrdCol::Size, false)),
        ..OrdersViewState::default()
    };
    let mut rows = [entry(2, 1, "BBB", 1.0), entry(1, 2, "AAA", 1.0)];

    sort_entries(&mut rows, &view, &overlays);

    assert_eq!(
        rows.iter().map(|row| row.core).collect::<Vec<_>>(),
        vec![1, 2]
    );
}

/// `orders/sort.rs:sort_entries` must retain the old SellFirst/newest behavior without an override.
///
/// Mutation: route `header_sort = None` through a column comparator. The pending BUY would no
/// longer trail the executed SELL, changing every existing user's first-launch order.
#[test]
fn missing_header_override_keeps_the_legacy_default() {
    let overlays = HashMap::new();
    let executed = entry(1, 1, "AAA", 1.0);
    let mut pending = entry(1, 2, "BBB", 1.0);
    pending.row.status.clear();
    pending.row.filled = false;
    let mut rows = [pending, executed];

    sort_entries(&mut rows, &OrdersViewState::default(), &overlays);

    assert_eq!(
        rows.iter().map(|row| row.row.uid).collect::<Vec<_>>(),
        vec![1, 2]
    );
}
