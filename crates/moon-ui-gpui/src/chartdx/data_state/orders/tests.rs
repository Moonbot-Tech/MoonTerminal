//! Synthetic timing of order captions against a core's retained closed-order history.

use super::build_order_labels;
use moon_core::feed::{OrderRow, OrderTrace, OrderTracePoint};
use moon_core::session::order_lines::OrderLineStore;

/// Construct neutral live/terminal rows without a session, GPU, locale or production data.
fn row(uid: u64, closed: bool) -> OrderRow {
    OrderRow {
        market: "XALPHAUSDT".into(),
        market_display: "XALPHAUSDT".into(),
        coin: "XALPHA".into(),
        quote: "USDT".into(),
        is_short: uid.is_multiple_of(2),
        size: 1.0,
        remaining_size: 1.0,
        sl_on: true,
        ts_on: false,
        vstop_on: false,
        sl_fixed: false,
        ts_fixed: false,
        vstop_fixed: false,
        vstop_level: 0.0,
        vstop_vol: 0.0,
        buy_price: 10.0,
        sell_price: 12.0,
        create_time_ms: 1_000.0,
        sell_create_time_ms: 2_000.0,
        entry_fill_time_ms: 1_500.0,
        price: 11.0,
        fill_pct: 100.0,
        strat: "UNIT_01".into(),
        strat_name: String::new(),
        strat_id: 0,
        status: if closed { "SellDone" } else { "SellSet" }.into(),
        uid,
        emulator: false,
        job_is_done: closed,
        pending: false,
        filled: true,
        stop_loss: Some(9.0),
        trailing: None,
        take_profit: None,
        vstop: None,
        pending_cond: None,
        liq: None,
        panic_sell: false,
        is_moon_shot: false,
        corridor_price_down: 0.0,
        corridor_price_up: 0.0,
        buy_trace: Some(OrderTrace {
            points: (0..(50 + uid as usize % 151))
                .map(|i| OrderTracePoint {
                    time_ms: 1_000.0 + i as f64,
                    price: 10.0 + i as f32 * 0.01,
                })
                .collect(),
            ..Default::default()
        }),
        sell_trace: None,
    }
}

/// Time caption construction for 10 cores, each holding 200 open and 5000 closed orders.
#[test]
#[ignore = "manual heavy-user timing probe"]
fn bench_goal_b_order_labels_revision() {
    use std::{hint::black_box, time::Instant};
    let stores: Vec<_> = (0..10)
        .map(|_| {
            let mut store = OrderLineStore::default();
            store.update(&(0..5_000).map(|uid| row(uid, true)).collect::<Vec<_>>(), 0);
            store.update(
                &(5_000..5_200)
                    .map(|uid| row(uid, false))
                    .collect::<Vec<_>>(),
                0,
            );
            store
        })
        .collect();
    let theme = moon_core::config::ChartTheme::default();
    let (mut labels, mut book) = (Vec::new(), Vec::new());
    let start = Instant::now();
    for _ in 0..30 {
        for store in &stores {
            build_order_labels(
                &mut labels,
                &mut book,
                black_box(store),
                "XALPHAUSDT",
                &theme,
                Some(1.0),
                None,
                None,
            );
            black_box((&labels, &book));
        }
    }
    println!(
        "goal_b_order_labels_10_cores: {} ns/iter",
        start.elapsed().as_nanos() / 30
    );
}

/// Time one sell-line depth pass over a deep book and hundreds of open orders.
///
/// Each label used to scan the whole book. The chart asks for this on a book revision, on the UI
/// thread, once per frame while an order line is dragged.
#[test]
#[ignore = "manual heavy-user timing probe"]
fn bench_goal_e_orderbook_notionals() {
    use std::hint::black_box;
    use std::time::Instant;

    use moon_core::data::OrderBookModel;
    use moon_core::feed::{Level, OrderBook};

    use crate::chartdx::hits::OrderBookLabel;

    let book = OrderBook {
        bids: (0..500)
            .map(|i| Level {
                price: 100.0 - i as f32 * 0.01,
                qty: 1.0 + (i % 7) as f32,
            })
            .collect(),
        asks: (0..500)
            .map(|i| Level {
                price: 100.05 + i as f32 * 0.01,
                qty: if i == 10 {
                    f32::NAN
                } else {
                    1.0 + (i % 5) as f32
                },
            })
            .collect(),
    };
    let mut model = OrderBookModel::default();
    model.update(&book);
    let mut labels: Vec<OrderBookLabel> = (0..200)
        .map(|i| OrderBookLabel {
            uid: i,
            interacting: false,
            price: 99.0 + i as f32 * 0.02,
            short: i % 2 == 0,
            notional: None,
        })
        .collect();
    let start = Instant::now();
    for _ in 0..50 {
        super::refresh_orderbook_label_notionals(black_box(&mut labels), black_box(&model));
        black_box(&labels);
    }
    println!(
        "goal_e_notionals_200_labels_1000_levels: {} ns/iter",
        start.elapsed().as_nanos() / 50
    );
}
