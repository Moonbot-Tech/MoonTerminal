//! Synthetic queue load and ordering checks for a stalled session drain.

use crate::feed::{FeedMsg, OrderRow, OrderTrace, OrderTracePoint, trade_sound_test_feed};

/// Supplies two trace legs and independently owned labels, matching a busy open order.
fn row(uid: u64) -> OrderRow {
    OrderRow {
        market: "XALPHAUSDT".into(),
        market_display: "XALPHAUSDT".into(),
        coin: "XALPHA".into(),
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
        buy_price: 100.0,
        sell_price: 102.0,
        create_time_ms: 1000.0,
        sell_create_time_ms: 2000.0,
        entry_fill_time_ms: 1500.0,
        price: 101.0,
        fill_pct: 100.0,
        strat: "UNIT_01".into(),
        strat_name: "UNIT_01".into(),
        strat_id: 1,
        status: "SellSet".into(),
        uid,
        emulator: true,
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
        buy_trace: Some(trace(50)),
        sell_trace: Some(trace(200)),
    }
}

/// Makes trace history large enough that retained full snapshots dominate queue ownership.
fn trace(points: usize) -> OrderTrace {
    OrderTrace {
        points: (0..points)
            .map(|i| OrderTracePoint {
                time_ms: 1000.0 + i as f64,
                price: 100.0 + i as f32,
            })
            .collect(),
        ..Default::default()
    }
}

/// Counts heap bytes owned by the projected rows; excludes allocator and channel overhead.
fn row_bytes(row: &OrderRow) -> usize {
    [
        &row.market,
        &row.market_display,
        &row.coin,
        &row.quote,
        &row.strat,
        &row.strat_name,
        &row.status,
    ]
    .into_iter()
    .map(String::capacity)
    .sum::<usize>()
        + [&row.buy_trace, &row.sell_trace]
            .into_iter()
            .flatten()
            .map(|trace| trace.points.capacity() * size_of::<OrderTracePoint>())
            .sum::<usize>()
}

/// Counts snapshot and captured-turn ownership without confusing logical bytes with process RSS.
fn message_bytes(msg: &FeedMsg) -> usize {
    match msg {
        FeedMsg::Orders(batch) | FeedMsg::OrderLines(batch) => {
            batch.rows.capacity() * size_of::<OrderRow>()
                + batch.rows.iter().map(row_bytes).sum::<usize>()
                + batch.captured_turns.capacity() * size_of::<Vec<OrderRow>>()
                + batch
                    .captured_turns
                    .iter()
                    .map(|turn| {
                        turn.capacity() * size_of::<OrderRow>()
                            + turn.iter().map(row_bytes).sum::<usize>()
                    })
                    .sum::<usize>()
        }
        _ => 0,
    }
}

/// Simulates two seconds without a drain at ten cores x 20 Hz x 200 orders, without sleeping.
#[test]
#[ignore = "manual queue ownership and send-latency scale measurement"]
fn bench_stalled_order_queue() {
    let feeds: Vec<_> = (0..10).map(|_| trade_sound_test_feed()).collect();
    let rows: Vec<_> = (1..=200).map(row).collect();
    let mut samples = Vec::new();
    for tick in 0usize..40 {
        for (tx, _) in &feeds {
            let msg = if tick.is_multiple_of(5) {
                FeedMsg::Orders(rows.clone().into())
            } else {
                FeedMsg::OrderLines(rows.clone().into())
            };
            let start = std::time::Instant::now();
            tx.send(msg).unwrap();
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    let mut bytes = 0;
    let mut messages = 0;
    for (_, feed) in &feeds {
        while let Ok(msg) = feed.rx.try_recv() {
            crate::feed::note_drained();
            bytes += message_bytes(&msg);
            messages += 1;
        }
    }
    assert_eq!(
        messages, 400,
        "a full snapshot was lost during the stalled drain"
    );
    // Seven independent labels total 51 bytes; the two trace legs contain 250 points.
    let expected = 400 * 200 * (size_of::<OrderRow>() + 51 + 250 * size_of::<OrderTracePoint>());
    assert_eq!(bytes, expected);
    samples.sort_by(f64::total_cmp);
    println!(
        "[bench] stalled queue messages={messages} owned_row_bytes={bytes} send median={:.6} ms p95={:.6} ms",
        samples[200], samples[380]
    );
}

/// Dropping a superseded full snapshot loses terminal orders that arrived without captured turns.
/// The queued path must retain their chart history and both table-publication revisions.
#[test]
fn stalled_drain_retains_snapshot_only_terminal_orders() {
    use crate::session::order_lines::LineKind;
    use crate::session::store::CoreData;

    let (tx, feed) = trade_sound_test_feed();
    let mut terminal = row(1);
    terminal.job_is_done = true;
    terminal.status = "Done".into();
    let sequence = [
        FeedMsg::Orders(vec![terminal].into()),
        FeedMsg::StrategiesRunning(true),
        FeedMsg::OrderLines(Vec::new().into()),
        FeedMsg::StrategiesRunning(false),
        FeedMsg::Orders(vec![row(2)].into()),
        FeedMsg::StrategiesRunning(true),
    ];
    let mut reference = CoreData::new();
    for message in sequence {
        reference.apply(message.clone());
        tx.send(message).unwrap();
    }
    let mut actual = CoreData::new();
    while let Ok(message) = feed.rx.try_recv() {
        crate::feed::note_drained();
        actual.apply(message);
    }

    assert_eq!(actual.orders_table_rev, 2);
    assert_eq!(actual.orders_table_rev, reference.orders_table_rev);
    assert_eq!(actual.order_lines_rev, reference.order_lines_rev);
    assert_eq!(actual.strategies_running_rev, 3);
    assert_eq!(
        actual.strategies_running_rev,
        reference.strategies_running_rev
    );
    assert_eq!(actual.strategies_running, reference.strategies_running);
    assert_eq!(
        format!("{:?}", actual.orders),
        format!("{:?}", reference.orders)
    );
    assert_eq!(
        actual.order_lines.auto_fit_range("XALPHAUSDT"),
        reference.order_lines.auto_fit_range("XALPHAUSDT")
    );
    assert!(
        actual.order_lines.knows(1),
        "snapshot-only terminal history disappeared"
    );
    assert!(!actual.order_lines.order_state(1).unwrap().active);
    assert!(actual.order_lines.order_state(2).unwrap().active);
    let drawn = actual.order_lines.market_draw_orders("XALPHAUSDT", 2);
    let terminal = drawn.iter().find(|order| order.uid == 1).unwrap();
    assert_eq!(
        terminal.lines[LineKind::Buy as usize].server_points.len(),
        50
    );
    assert_eq!(
        terminal.lines[LineKind::Sell as usize].server_points.len(),
        200
    );
    for uid in [1, 2] {
        assert_eq!(
            actual.order_lines.current_line_price(uid, LineKind::Buy),
            reference.order_lines.current_line_price(uid, LineKind::Buy)
        );
        assert_eq!(
            actual.order_lines.current_line_price(uid, LineKind::Sell),
            reference
                .order_lines
                .current_line_price(uid, LineKind::Sell)
        );
    }
}
