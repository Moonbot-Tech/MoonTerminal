use super::*;

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
