//! Synthetic timing of order captions against a core's retained closed-order history.

use super::{build_order_labels, refresh_position_fills};
use crate::chartdx::PaneRender;
use moon_core::feed::{OrderRow, OrderTrace, OrderTracePoint};
use moon_core::session::order_lines::OrderLineStore;

/// The position fills follow the ROW TABLE's revision (#1402). A fill the line store learned first
/// reaches the rows only with the next table batch, which moves no order-line revision, so a
/// table revision that brings a fill must report a change — that is the only thing that rebuilds
/// the captions then. The same revision again must not re-walk the rows, and the switch off (or a
/// frozen viewer) clears the fills and forgets the revision so turning it back on reads afresh.
#[test]
fn position_fills_follow_the_row_table_revision() {
    let mut pr = PaneRender::new();
    let mut before_fill = row(1, false);
    before_fill.filled = false;
    before_fill.fill_pct = 0.0;
    before_fill.entry_fill_time_ms = 0.0;
    let walked = std::cell::Cell::new(0);
    let holds = || {
        walked.set(walked.get() + 1);
        true
    };
    assert!(!refresh_position_fills(
        &mut pr,
        true,
        7,
        &[before_fill],
        "XALPHAUSDT",
        holds
    ));
    assert!(pr.order_age_fills.is_empty());

    let filled = [row(1, false)];
    assert!(
        refresh_position_fills(&mut pr, true, 8, &filled, "XALPHAUSDT", holds),
        "the table batch that brings the fill must rebuild the captions"
    );
    assert_eq!(pr.order_age_fills, vec![(1, 1_500)]);
    let walks = walked.get();
    assert!(!refresh_position_fills(
        &mut pr,
        true,
        8,
        &filled,
        "XALPHAUSDT",
        holds
    ));
    assert_eq!(
        walked.get(),
        walks,
        "an unchanged table is not walked again"
    );

    assert!(refresh_position_fills(
        &mut pr,
        false,
        8,
        &filled,
        "XALPHAUSDT",
        holds
    ));
    assert!(pr.order_age_fills.is_empty());
    assert!(
        refresh_position_fills(&mut pr, true, 8, &filled, "XALPHAUSDT", holds),
        "back on, the same table revision is read afresh"
    );
}

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
                &[],
                0,
            );
            black_box((&labels, &book));
        }
    }
    println!(
        "goal_b_order_labels_10_cores: {} ns/iter",
        start.elapsed().as_nanos() / 30
    );
}

/// Only the ENTRY line of an order that holds a position states its age (#1402), as `age [N]`;
/// a resting entry keeps its size, and an order's exit and stop captions never carry an age.
/// Dropping the `age` branch from the entry text, or attaching the age to another line, breaks it.
#[test]
fn only_a_holders_entry_line_states_its_age() {
    let _locale = crate::test_locale::force("ru");
    let mut held = row(1, false);
    held.entry_fill_time_ms = 100_000.0;
    let mut resting = row(3, false);
    resting.filled = false;
    resting.fill_pct = 0.0;
    resting.entry_fill_time_ms = 0.0;
    let rows = vec![held, resting];
    let mut store = OrderLineStore::default();
    store.update(&rows, 0);
    let fills = crate::chartdx::text::position_fills(&rows, "XALPHAUSDT");
    let theme = moon_core::config::ChartTheme::default();
    let (mut labels, mut book) = (Vec::new(), Vec::new());
    build_order_labels(
        &mut labels,
        &mut book,
        &store,
        "XALPHAUSDT",
        &theme,
        None,
        None,
        None,
        &fills,
        142_000,
    );
    let aged: Vec<_> = labels.iter().filter(|l| l.age.is_some()).collect();
    assert_eq!(aged.len(), 1, "one holder, one aged caption");
    assert_eq!(
        (aged[0].uid, aged[0].priority),
        (1, crate::chartdx::PRIO_BUY)
    );
    let tag = &aged[0].age.as_ref().expect("aged").tag;
    assert!(!tag.is_empty(), "the order has a chart number");
    assert_eq!(aged[0].text, format!("42с {tag}"));
    let resting_entry = labels
        .iter()
        .find(|l| l.uid == 3 && l.priority == crate::chartdx::PRIO_BUY)
        .expect("a resting entry keeps its caption");
    assert!(resting_entry.age.is_none());
    assert!(
        !resting_entry.text.contains('с'),
        "{:?}",
        resting_entry.text
    );

    // Switched off — the sync hands no fills — the holder's entry line is its bare tag again.
    build_order_labels(
        &mut labels,
        &mut book,
        &store,
        "XALPHAUSDT",
        &theme,
        None,
        None,
        None,
        &[],
        142_000,
    );
    assert!(labels.iter().all(|l| l.age.is_none()));
    let entry = labels
        .iter()
        .find(|l| l.uid == 1 && l.priority == crate::chartdx::PRIO_BUY)
        .expect("the entry caption");
    assert!(entry.text.starts_with('['), "{:?}", entry.text);
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
