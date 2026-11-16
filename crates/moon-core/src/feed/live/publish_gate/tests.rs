use super::*;

/// A one-shot identity gate would miss late currencies and leave changed bases on the old tape.
#[test]
fn account_base_publishes_only_nonempty_changes() {
    assert_eq!(base_to_publish(None, None), None);
    assert_eq!(base_to_publish(None, Some("")), None);
    assert_eq!(base_to_publish(None, Some("USDT")), Some("USDT".into()));
    assert_eq!(base_to_publish(Some("USDT"), Some("USDT")), None);
    assert_eq!(base_to_publish(Some("USDT"), Some("")), None);
    assert_eq!(
        base_to_publish(Some("USDT"), Some("USDC")),
        Some("USDC".into())
    );
}

/// Removing the cooldown rebuilds on every 5 ms event instead of publishing at 0/50/100/150/200.
#[test]
fn lines_burst_is_leading_edge_and_coalesced() {
    let start = Instant::now();
    let mut lines = deadline::CoalescedDeadline::new(ORDER_LINES_PERIOD);
    let mut published = Vec::new();
    for ms in (0..=200).step_by(5) {
        let now = start + Duration::from_millis(ms);
        lines.queue(now);
        if OrdersPublish::decide(false, lines.is_due(now)).is_some() {
            published.push(ms);
            lines.mark_attempt(now);
        }
    }
    assert_eq!(published, [0, 50, 100, 150, 200]);
}

/// Ignoring a queued line deadline in the sleep fold loses the trailing publish on a quiet core.
#[test]
fn trailing_lines_publish_without_another_event() {
    let start = Instant::now();
    let mut lines = deadline::CoalescedDeadline::new(ORDER_LINES_PERIOD);
    lines.queue(start);
    lines.mark_attempt(start);
    lines.queue(start + Duration::from_millis(5));
    let wait = lines.wait(start + Duration::from_millis(5)).unwrap();
    assert_eq!(wait, Duration::from_millis(45));
    assert_eq!(
        OrdersPublish::decide(false, lines.is_due(start + Duration::from_millis(50))),
        Some(OrdersPublish::Lines)
    );
}

/// A missing snapshot must retain queued line work without a zero-duration busy loop.
#[test]
fn missing_snapshot_defers_lines_and_table() {
    let now = Instant::now();
    let mut lines = deadline::CoalescedDeadline::new(ORDER_LINES_PERIOD);
    let mut table = deadline::CoalescedDeadline::new(ORDERS_TABLE_PERIOD);
    lines.queue(now);
    table.queue(now);
    assert_eq!(
        OrdersPublish::decide(table.is_due(now), lines.is_due(now)),
        Some(OrdersPublish::Table)
    );
    lines.defer(now);
    table.defer(now);
    assert!(lines.is_queued());
    assert_eq!(lines.wait(now), Some(Duration::from_millis(50)));
    assert_eq!(table.wait(now), Some(Duration::from_millis(250)));
}

/// #1081: while the consumer holds this core's previous batch the feed thread may not send the
/// table that is due. A zero wait for that table would turn the loop without a pause for the
/// whole stall, on every core at once. Dropping the hold from `order_waits` is the edit this
/// catches.
#[test]
fn a_held_publication_is_not_a_deadline_to_sleep_on() {
    let now = Instant::now();
    let mut table = deadline::CoalescedDeadline::new(ORDERS_TABLE_PERIOD);
    let mut lines = deadline::CoalescedDeadline::new(ORDER_LINES_PERIOD);
    table.queue(now);
    lines.queue(now);

    assert_eq!(
        order_waits(&table, &lines, now, || true),
        (Some(Duration::ZERO), Some(Duration::ZERO)),
        "with room for it, a due publication runs on the next turn"
    );
    assert_eq!(order_waits(&table, &lines, now, || false), (None, None));
}

/// The hold is for a publication that is due. One still inside its cooldown keeps its deadline,
/// or a table queued while the consumer was briefly behind would wait for an unrelated wake.
/// The gate is not even asked then: a refusal arms a wake, and one nobody needs is a wasted turn.
#[test]
fn a_publication_still_cooling_down_keeps_its_deadline() {
    let now = Instant::now();
    let mut table = deadline::CoalescedDeadline::new(ORDERS_TABLE_PERIOD);
    let lines = deadline::CoalescedDeadline::new(ORDER_LINES_PERIOD);
    table.mark_attempt(now);
    table.queue(now);

    let waits = order_waits(&table, &lines, now, || {
        panic!("nothing is due, the gate must not be asked")
    });

    assert_eq!(waits, (Some(ORDERS_TABLE_PERIOD), None));
}

/// The publication step as the loop runs it, against a consumer that stops reading for two
/// seconds: one table goes out, none follows while it sits in the channel, and the first turn
/// after the take sends the owed one.
#[test]
fn the_owed_table_goes_out_on_the_turn_after_the_take() {
    let (tx, feed, wakes) = crate::feed::order_gate_test_feed();
    let start = Instant::now();
    let mut table = deadline::CoalescedDeadline::new(ORDERS_TABLE_PERIOD);
    let mut sent_at = Vec::new();
    for ms in (0..=2000).step_by(50).chain([2010]) {
        if ms == 2010 {
            assert_eq!(sent_at, [0], "tables went out with nobody reading");
            assert!(feed.rx.try_recv().is_ok());
            assert!(wakes.try_recv().is_ok(), "the feed thread was not woken");
        }
        let now = start + Duration::from_millis(ms);
        table.queue(now);
        let publish =
            OrdersPublish::decide(table.is_due(now), false).filter(|_| tx.order_batch_admitted());
        if publish.is_some() {
            table.mark_attempt(now);
            tx.send(FeedMsg::Orders(Vec::<crate::feed::OrderRow>::new().into()))
                .unwrap();
            sent_at.push(ms);
        }
    }
    assert_eq!(sent_at, [0, 2010]);
}

/// Removing the restart gate would restore the exited letter; collapsing empty suffixes would
/// lose the distinction between a reported release and an unknown build.
#[test]
fn restart_letter_is_unknown_but_fresh_letters_are_preserved() {
    assert_eq!(letter_to_publish(Some("R3".into()), true), None);
    assert_eq!(letter_to_publish(Some(String::new()), true), None);
    assert_eq!(
        letter_to_publish(Some("R3".into()), false),
        Some("R3".into())
    );
    assert_eq!(
        letter_to_publish(Some(String::new()), false),
        Some(String::new())
    );
    assert_eq!(letter_to_publish(None, false), None);
}

/// `publish_gate.rs:server_info_predates_restart` must withhold only the allocation captured at
/// `ServerRestart`. Treating every snapshot as stale would blank the build after an ordinary
/// reconnect; treating none as stale would republish the exited process's letter.
///
/// Pinning that allocation is the caller's held `Arc<MoonStateSnapshot>` (`PinnedRestartInfo` in
/// `feed/live/mod.rs`). A unit test cannot force the allocator to reuse a freed address, so the
/// pin itself is not exercised here.
#[test]
fn a_server_restart_withholds_only_the_server_info_it_observed() {
    assert!(server_info_predates_restart(0x1000, Some(0x1000)));
    assert!(!server_info_predates_restart(0x2000, Some(0x1000)));
    assert!(
        !server_info_predates_restart(0x1000, None),
        "a same-process reconnect has no restart mark and must republish"
    );
}

/// Dropping the station gate would send a station the 50 ms line copies again, twice the tables'
/// count; dropping the table debt would leave a trace-only turn's captured rows waiting for an
/// unrelated table event.
#[test]
fn a_station_line_event_owes_the_table_and_no_line_copy() {
    assert_eq!(
        OrderDebts::of_turn(true, false, true),
        OrderDebts {
            table: true,
            lines: false
        }
    );
}

/// The terminal's chart keeps its line copies, and a trace point alone does not republish the
/// table there.
#[test]
fn a_terminal_trace_point_owes_only_the_line_copies() {
    assert_eq!(
        OrderDebts::of_turn(false, false, true),
        OrderDebts {
            table: false,
            lines: true
        }
    );
}

/// A table event owes the table everywhere; the station gate must not swallow it, and the
/// terminal keeps the copy beside it.
#[test]
fn a_table_event_owes_the_table_on_both_hosts() {
    assert!(OrderDebts::of_turn(true, true, true).table);
    assert_eq!(
        OrderDebts::of_turn(false, true, true),
        OrderDebts {
            table: true,
            lines: true
        }
    );
}
