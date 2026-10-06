use super::*;

/// Preserves the old restart-at-one assignment as an independent chart-label oracle.
fn reference_chart_numbers(store: &OrderLineStore, rows: &[OrderRow]) -> HashMap<u64, u32> {
    let mut new_nums = HashMap::new();
    let mut fresh: Vec<&OrderRow> = rows
        .iter()
        .filter(|r| !store.orders.contains_key(&r.uid))
        .collect();
    let mut used: HashMap<&str, HashSet<u32>> = HashMap::new();
    for uid in &store.open_uids {
        if let Some(order) = store.orders.get(uid) {
            if order.chart_num > 0 {
                used.entry(order.market.as_str())
                    .or_default()
                    .insert(order.chart_num);
            }
        }
    }
    fresh.sort_by_key(|r| r.uid);
    for r in fresh {
        let set = used.entry(r.market.as_str()).or_default();
        let mut n = 1u32;
        while set.contains(&n) {
            n += 1;
        }
        set.insert(n);
        new_nums.insert(r.uid, n);
    }
    new_nums
}

/// Skipping a hole or sharing a cursor between markets would change the chart's order labels.
#[test]
fn chart_numbers_match_reference_assignment() {
    let mut store = OrderLineStore::default();
    let mut rows = bench_rows(21, 3);
    store.update(&rows, 0);
    for row in &mut rows {
        let slot = store.orders[&row.uid].chart_num;
        row.job_is_done = ![1, 2, 4, 7].contains(&slot);
    }
    store.update(&rows, 0);
    for market in 0..3 {
        let mut occupied: Vec<_> = store
            .orders
            .values()
            .filter(|o| o.market == format!("SYN{market:03}") && o.closed_ms.is_none())
            .map(|o| o.chart_num)
            .collect();
        occupied.sort_unstable();
        assert_eq!(occupied, [1, 2, 4, 7]);
    }
    for batch in 0..4 {
        let mut fresh = bench_rows(18, 3);
        for row in &mut fresh {
            row.uid += 100 + batch * 18;
            row.job_is_done = row.uid % 5 == 0;
        }
        fresh.reverse();
        rows.extend(fresh);
        let mut expected: HashMap<_, _> = store
            .orders
            .iter()
            .map(|(uid, order)| (*uid, order.chart_num))
            .collect();
        expected.extend(reference_chart_numbers(&store, &rows));
        store.update(&rows, 0);
        for (uid, chart_num) in expected {
            assert_eq!(store.orders[&uid].chart_num, chart_num, "uid {uid}");
        }
        for row in &mut rows {
            if row.uid % 4 == batch {
                row.job_is_done = true;
            }
        }
        store.update(&rows, 0);
    }
}

/// Preserves the old full-store scan and closed-ring traversal as the draw-query oracle.
fn reference_market_draw_orders<'a>(
    store: &'a OrderLineStore,
    market: &str,
    max_closed: usize,
) -> Vec<&'a RetainedOrder> {
    let mut out: Vec<&RetainedOrder> = store
        .orders
        .values()
        .filter(|o| o.market == market && o.closed_ms.is_none())
        .collect();
    let mut taken = 0usize;
    let mut seen: HashSet<u64> = HashSet::new();
    for uid in store.closed_ring.iter().rev() {
        if taken >= max_closed {
            break;
        }
        if !seen.insert(*uid) {
            continue;
        }
        if let Some(o) = store.orders.get(uid) {
            if o.closed_ms.is_some() && o.market == market {
                out.push(o);
                taken += 1;
            }
        }
    }
    out
}

/// Compares the same open set by sorted (seq, uid) pairs and preserves the exact closed suffix.
fn assert_market_draw_reference(store: &OrderLineStore) {
    for market in ["SYN000", "SYN001", "SYN002", "absent"] {
        for cap in [0, 1, 50, CLOSED_RING_CAP + 1] {
            let actual = store.market_draw_orders(market, cap);
            let expected = reference_market_draw_orders(store, market, cap);
            let open = |orders: &[&RetainedOrder]| {
                let mut ids: Vec<_> = orders
                    .iter()
                    .filter(|o| o.closed_ms.is_none())
                    .map(|o| o.uid)
                    .collect();
                ids.sort_unstable();
                ids
            };
            assert_eq!(open(&actual), open(&expected), "market {market}, cap {cap}");
            let sequence = |orders: &[&RetainedOrder]| {
                let mut ids: Vec<_> = orders
                    .iter()
                    .filter(|o| o.closed_ms.is_none())
                    .map(|o| (o.seq, o.uid))
                    .collect();
                ids.sort_unstable();
                ids
            };
            assert_eq!(sequence(&actual), sequence(&expected));
            let closed = |orders: &[&RetainedOrder]| {
                orders
                    .iter()
                    .filter(|o| o.closed_ms.is_some())
                    .map(|o| o.uid)
                    .collect::<Vec<_>>()
            };
            assert_eq!(closed(&actual), closed(&expected));
            assert!(
                actual
                    .iter()
                    .skip_while(|o| o.closed_ms.is_none())
                    .all(|o| o.closed_ms.is_some())
            );
        }
    }
}

/// Missing index maintenance on insertion, closure, or revival would hide or duplicate chart lines.
#[test]
fn market_draw_orders_matches_reference() {
    let mut store = OrderLineStore::default();
    let mut rows = bench_rows(12, 3);
    rows[0].job_is_done = true;
    store.update(&rows, 0);
    assert_market_draw_reference(&store);

    rows[1].job_is_done = true;
    store.update(&rows, 0);
    assert_market_draw_reference(&store);

    let missing = rows.remove(2);
    store.orders.get_mut(&missing.uid).unwrap().last_seen_ms -= CLOSE_GRACE_MS + 1.0;
    store.update(&rows, 0);
    assert_eq!(
        store.order_state(missing.uid).unwrap().closed_reason,
        Some(OrderCloseReason::BackstopMissing)
    );
    assert_market_draw_reference(&store);

    rows[0].job_is_done = false;
    rows[1].job_is_done = false;
    rows.push(missing);
    store.update(&rows, 0);
    assert_market_draw_reference(&store);
    for row in rows.iter_mut().filter(|r| r.uid < 3) {
        row.job_is_done = true;
    }
    store.update(&rows, 0);
    assert_market_draw_reference(&store);

    // Closure cleanup removes this UID from open_uids even though the final row revives it.
    rows[0].job_is_done = false;
    store.update(&rows, 0);
    assert_market_draw_reference(&store);
    rows[0].job_is_done = true;
    let mut revived = rows[0].clone();
    revived.job_is_done = false;
    rows.push(revived);
    store.update(&rows, 0);
    assert!(store.order_state(0).unwrap().active);
    assert_market_draw_reference(&store);
    rows.pop();

    let mut terminal = bench_rows(CLOSED_RING_CAP as u64 + 1, 3);
    for row in &mut terminal {
        row.uid += 100;
        row.job_is_done = true;
    }
    rows.extend(terminal);
    store.update(&rows, 0);
    assert_eq!(store.closed_ring.len(), CLOSED_RING_CAP);
    assert!(!store.knows(0));
    assert_market_draw_reference(&store);
}

/// Generates seeded synthetic rows using the existing order constructor.
fn bench_rows(count: u64, markets: u64) -> Vec<OrderRow> {
    let mut seed = 0x1234_abcd_u64;
    (0..count)
        .map(|uid| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let mut row = order(uid);
            row.market = format!("SYN{:03}", uid % markets);
            row.market_display = row.market.clone();
            row.coin = "SYN".into();
            row.strat = "synthetic".into();
            row.buy_price = 100.0 + (seed % 1000) as f64 / 100.0;
            row.price = row.buy_price as f32;
            row
        })
        .collect()
}

/// Measures first-snapshot chart-number assignment with one dense and many small markets.
#[test]
#[ignore]
fn bench_first_snapshot_chart_numbers() {
    for (count, markets, label) in [(5000, 1, "5k one market"), (5100, 300, "300 markets x 17")] {
        let rows = bench_rows(count, markets);
        let mut samples = Vec::new();
        for _ in 0..50 {
            let mut store = OrderLineStore::default();
            let start = std::time::Instant::now();
            std::hint::black_box(store.update(std::hint::black_box(&rows), 0));
            samples.push(start.elapsed().as_micros());
            std::hint::black_box(store);
        }
        samples.sort_unstable();
        println!(
            "[bench] B2 first_snapshot {label}: median {} us",
            samples[25]
        );
    }
}

/// Measures each draw query over 5k open rows and a 5k retained closed ring.
#[test]
#[ignore]
fn bench_market_draw_orders_10k() {
    let mut rows = bench_rows(10000, 300);
    let mut store = OrderLineStore::default();
    store.update(&rows, 0);
    for row in &mut rows[5000..] {
        row.job_is_done = true;
    }
    store.update(&rows, 0);
    assert_eq!(store.open_uids.len(), 5000);
    assert_eq!(store.closed_ring.len(), 5000);
    let mut samples = Vec::new();
    for _ in 0..50 {
        let start = std::time::Instant::now();
        for market in 0..300 {
            let name = format!("SYN{market:03}");
            std::hint::black_box(store.market_draw_orders(std::hint::black_box(&name), 50));
        }
        samples.push(start.elapsed().as_secs_f64() * 1_000_000.0 / 300.0);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "[bench] B3 market_draw_orders 10k max_closed=50: median {:.3} us/call",
        samples[25]
    );
}

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
        sell_create_time_ms: 0.0,
        entry_fill_time_ms: 0.0,
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

/// First step time of one retained line, which is where the chart starts drawing it.
fn line_start(store: &OrderLineStore, uid: u64, kind: LineKind) -> f64 {
    store
        .orders
        .get(&uid)
        .expect("retained order must exist")
        .lines[kind as usize]
        .steps
        .first()
        .expect("line must have a first step")
        .0
}

/// A filled order holding a position, with the leg times a live core supplies.
fn filled_order(uid: u64, now: f64) -> OrderRow {
    let mut r = order(uid);
    r.create_time_ms = now - 600_000.0;
    r.entry_fill_time_ms = now - 400_000.0;
    r.sell_create_time_ms = now - 300_000.0;
    r.filled = true;
    r.fill_pct = 100.0;
    r.sell_price = 61_000.0;
    r.stop_loss = Some(59_000.0);
    r
}

#[test]
fn missing_order_does_not_close_without_terminal_status_or_backstop_grace() {
    let mut store = OrderLineStore::default();
    assert!(store.update(&[order(42)], 0));

    assert!(!store.update(&[], 0));

    let state = store.order_state(42).expect("retained order must stay");
    assert_eq!(state.closed_reason, None);
    assert!(state.closed_store_ms.is_none());
    assert!(state.closed_rev.is_none());
    assert!(state.active);
}

#[test]
fn terminal_status_closes_order_immediately() {
    let mut store = OrderLineStore::default();
    assert!(store.update(&[order(42)], 0));

    let mut done = order(42);
    done.job_is_done = true;
    assert!(store.update(&[done], 0));

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
    assert!(store.update(&[order(42)], 0));
    assert_eq!(store.auto_fit_range("BTCUSDT"), Some((60_000.0, 60_000.0)));

    let mut done = order(42);
    done.job_is_done = true;
    assert!(store.update(&[done], 0));
    assert_eq!(store.auto_fit_range("BTCUSDT"), None);

    assert!(store.update(&[order(42)], 0));
    assert_eq!(store.auto_fit_range("BTCUSDT"), Some((60_000.0, 60_000.0)));
}

/// `order_lines.rs:update` marking every open UID as seen without comparing snapshot generations
/// would keep an order missing beyond the grace period visible forever.
#[test]
fn missing_order_closes_after_backstop_grace() {
    let mut store = OrderLineStore::default();
    assert!(store.update(&[order(42)], 0));
    store
        .orders
        .get_mut(&42)
        .expect("retained order must exist")
        .last_seen_ms -= CLOSE_GRACE_MS + 1.0;

    assert!(store.update(&[], 0));

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
    assert!(store.update(&[order(20), order(10)], 0));
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
    assert!(store.update(&[order(20), done], 0));
    assert!(store.update(&[order(20), order(30)], 0));

    assert_eq!(
        store
            .orders
            .get(&30)
            .expect("replacement order must exist")
            .chart_num,
        1
    );
}

/// `order_lines.rs:update` starting the exit line at the local clock dated the sell of every
/// position older than the process to the terminal's launch, while its entry stayed correct.
#[test]
fn sell_and_stop_lines_start_at_their_wire_times() {
    let now = now_unix_ms();
    let mut store = OrderLineStore::default();
    assert!(store.update(&[filled_order(42, now)], 0));

    assert_eq!(line_start(&store, 42, LineKind::Buy), now - 600_000.0);
    assert_eq!(line_start(&store, 42, LineKind::Sell), now - 300_000.0);
    assert_eq!(line_start(&store, 42, LineKind::Stop), now - 400_000.0);
}

/// An entry leg reporting no close time — a position inherited without a buy leg — still must not
/// drop its stop line onto the local clock while the exit leg knows when the position opened.
#[test]
fn stop_line_falls_back_to_the_exit_leg_when_the_fill_time_is_absent() {
    let now = now_unix_ms();
    let mut store = OrderLineStore::default();
    let mut r = filled_order(42, now);
    r.entry_fill_time_ms = 0.0;
    assert!(store.update(&[r], 0));

    assert_eq!(line_start(&store, 42, LineKind::Stop), now - 300_000.0);
}

/// With no wire time at all the lines keep their previous behaviour — start here and now — rather
/// than collapsing onto the epoch and stretching a line across the whole chart.
#[test]
fn lines_without_wire_times_start_at_the_local_clock() {
    let now = now_unix_ms();
    let mut store = OrderLineStore::default();
    let mut r = filled_order(42, now);
    r.entry_fill_time_ms = 0.0;
    r.sell_create_time_ms = 0.0;
    assert!(store.update(&[r], 0));

    for kind in [LineKind::Sell, LineKind::Stop] {
        let start = line_start(&store, 42, kind);
        assert!(
            start >= now && start - now < 60_000.0,
            "{kind:?} line must start at the local clock, got {start} against {now}"
        );
    }
}

/// An order the core dates not at all — a sale from an already-held asset, which has no buy leg —
/// falls back to the local clock for its own creation. Using that fallback as the floor for the
/// other lines would re-anchor the exit to the terminal's launch, which is the whole defect.
#[test]
fn an_order_without_a_creation_time_still_dates_its_exit_from_the_wire() {
    let now = now_unix_ms();
    let mut store = OrderLineStore::default();
    let mut r = filled_order(42, now);
    r.create_time_ms = 0.0;
    r.entry_fill_time_ms = 0.0;
    assert!(store.update(&[r], 0));

    assert_eq!(line_start(&store, 42, LineKind::Sell), now - 300_000.0);
}

/// The fill is when a protective line came into existence, so it outranks the exit leg's creation
/// even when the exit was placed first. Anchoring a stop to the earlier of the two would draw it
/// across a window in which it did not exist.
#[test]
fn the_fill_outranks_the_exit_leg_for_the_stop_lines() {
    let now = now_unix_ms();
    let mut store = OrderLineStore::default();
    let mut r = filled_order(42, now);
    r.sell_create_time_ms = now - 500_000.0;
    assert!(store.update(&[r], 0));

    assert_eq!(line_start(&store, 42, LineKind::Stop), now - 400_000.0);
    assert_eq!(line_start(&store, 42, LineKind::Sell), now - 500_000.0);
}

/// A first batch whose wire create time is AHEAD of the local clock is folded to `now_ms` by
/// `wire_line_start` — a stand-in, not a real wire time. A later adoption re-derives `create_ms`
/// from the corrected row and REBASES `steps[0]` onto that start.
///
/// `order_lines.rs:LineTrace::update` ignoring the `rebase` flag leaves the entry mark at the
/// folded stand-in; delta-shifting that stand-in (`folded + (0 - 2h)`) lands it two hours LEFT of
/// the candles, which is the reviewed failure `shift_wire_times` produced.
#[test]
fn a_folded_first_step_is_rebased_from_the_re_derived_start() {
    let mut store = OrderLineStore::default();
    let now1 = now_unix_ms();
    let mut folded = order(42);
    folded.create_time_ms = now1 + 500_000.0;
    assert!(store.update(&[folded], 0));
    let folded_create_ms = store.orders.get(&42).unwrap().create_ms;
    let folded_step0 = line_start(&store, 42, LineKind::Buy);
    assert!(
        (folded_create_ms - now1).abs() < 1_000.0,
        "a future wire time must fold to now, not read as-is"
    );
    assert!(
        (folded_step0 - folded_create_ms).abs() < 1.0,
        "the entry line's first step is the folded stand-in"
    );

    let mut corrected = order(42);
    let real_create_ms = now1 - 300_000.0;
    corrected.create_time_ms = real_create_ms;
    assert!(store.update(&[corrected], 1));

    let after_create = store.orders.get(&42).unwrap().create_ms;
    let after_step0 = line_start(&store, 42, LineKind::Buy);
    let shifted_stand_in = folded_step0 - 7_200_000.0;
    assert!(
        (after_create - real_create_ms).abs() < 1.0,
        "create_ms must re-derive from the corrected row, got {after_create}"
    );
    assert!(
        (after_step0 - real_create_ms).abs() < 1.0,
        "steps[0] must rebase onto the re-derived start, not stay folded, got {after_step0}"
    );
    assert!(
        (after_step0 - shifted_stand_in).abs() > 1_000.0,
        "must not land on folded + delta ({shifted_stand_in})"
    );
}

/// HIGH 2's pin: `wire_line_start` may fold a wire time that reads AHEAD of the local clock to
/// `now_ms` — a stand-in, not a real wire time. `shift_wire_times` no longer touches `create_ms` at
/// all; instead `update` RE-DERIVES it from each order's own row whenever its correction generation
/// is stale, so a later batch carrying the real (un-folded) time replaces the stand-in outright
/// rather than moving it by some delta that has no meaning for a value that was never a wire time.
#[test]
fn a_folded_start_is_re_derived_not_delta_shifted() {
    let mut store = OrderLineStore::default();
    let now1 = now_unix_ms();
    let mut folded = order(42);
    folded.create_time_ms = now1 + 500_000.0;
    assert!(store.update(&[folded], 0));
    let folded_create_ms = store.orders.get(&42).unwrap().create_ms;
    assert!(
        (folded_create_ms - now1).abs() < 1_000.0,
        "a future wire time must fold to now, not read as-is"
    );

    // The estimate is adopted (generation bumps to 1) and this order's row reappears carrying the
    // CORRECTED, un-folded wire time.
    let mut corrected = order(42);
    let real_create_ms = now1 - 300_000.0;
    corrected.create_time_ms = real_create_ms;
    assert!(store.update(&[corrected], 1));

    let after = store.orders.get(&42).unwrap().create_ms;
    assert!(
        (after - real_create_ms).abs() < 1.0,
        "must re-derive from the row rather than delta-shift the folded stand-in, got {after}"
    );
}

/// The clock-skew estimator gates its TIGHT sample class on this: a uid must read as unknown until
/// the store has actually retained it from a batch.
#[test]
fn knows_is_false_until_the_first_batch_retains_the_uid() {
    let mut store = OrderLineStore::default();
    assert!(!store.knows(42));

    assert!(store.update(&[order(42)], 0));

    assert!(store.knows(42));
}

#[test]
fn wire_line_start_stays_inside_the_order_lifetime() {
    let create = 1_000_000.0;
    let now = 2_000_000.0;

    assert_eq!(wire_line_start(1_500_000.0, create, now), 1_500_000.0);
    // Earlier than the order it belongs to, or past the right edge: pulled back to the bounds.
    assert_eq!(wire_line_start(900_000.0, create, now), create);
    assert_eq!(wire_line_start(9_000_000.0, create, now), now);
    // A local clock that stepped back behind the order's own creation must not panic or invert.
    assert_eq!(wire_line_start(9_000_000.0, create, 5_000.0), create);
    // Absent or unusable: reported as absent, not as a time near the epoch.
    assert_eq!(wire_line_start(0.0, create, now), 0.0);
    assert_eq!(wire_line_start(-1.0, create, now), 0.0);
    assert_eq!(wire_line_start(f64::NAN, create, now), 0.0);
    assert_eq!(wire_line_start(f64::INFINITY, create, now), 0.0);
}
