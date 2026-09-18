use super::*;

fn trace(own: bool, kind: ArchivedLineKind, points: &[(f64, f64)]) -> ArchivedOrderTrace {
    ArchivedOrderTrace {
        own,
        kind,
        stop_price: None,
        stop_time_ms: None,
        points: points.to_vec(),
    }
}

fn input() -> ArchivedOrdersInput<'static> {
    ArchivedOrdersInput {
        market: "ADAUSDT",
        is_short: false,
        quantity: 100.0,
        entry_fill_ms: Some(1_500.0),
        close_ms: 9_000.0,
        bright: false,
    }
}

#[test]
fn own_entry_and_exit_become_one_subject_order() {
    let store = OrderLineStore::archived(
        input(),
        &[
            trace(
                true,
                ArchivedLineKind::Entry,
                &[(1_000.0, 1.0), (1_200.0, 1.1)],
            ),
            trace(true, ArchivedLineKind::Exit, &[(2_000.0, 1.5)]),
        ],
    );
    let drawn = store.market_draw_orders("ADAUSDT", usize::MAX);
    assert_eq!(drawn.len(), 1);
    let order = drawn[0];
    assert!(order.subject);
    assert_eq!(order.create_ms, 1_000.0);
    assert_eq!(order.entry_fill_ms, Some(1_500.0));
    assert_eq!(order.closed_ms, Some(9_000.0));
    // The primary line sits at the LAST price from the FIRST instant; the path is the archive.
    assert_eq!(
        order.lines[LineKind::Buy as usize].steps,
        vec![(1_000.0, 1.1)]
    );
    assert_eq!(
        order.lines[LineKind::Buy as usize].server_points,
        vec![(1_000.0, 1.0), (1_200.0, 1.1)]
    );
    assert_eq!(
        order.lines[LineKind::Sell as usize].steps,
        vec![(2_000.0, 1.5)]
    );
    assert_eq!(store.rev, 1);
}

#[test]
fn inherited_and_repeated_own_lines_are_orders_of_their_own_and_pale() {
    let store = OrderLineStore::archived(
        input(),
        &[
            trace(true, ArchivedLineKind::Entry, &[(1_000.0, 1.0)]),
            trace(true, ArchivedLineKind::Entry, &[(1_100.0, 1.2)]),
            trace(false, ArchivedLineKind::Exit, &[(500.0, 0.9)]),
        ],
    );
    let mut drawn = store.market_draw_orders("ADAUSDT", usize::MAX);
    drawn.sort_by_key(|o| o.uid);
    assert_eq!(drawn.len(), 3);
    assert!(drawn[0].subject && drawn[0].uid == 1);
    assert!(!drawn[1].subject && !drawn[2].subject);
    // Only the subject's entry ends at the trade's fill.
    assert_eq!(drawn[0].entry_fill_ms, Some(1_500.0));
    assert_eq!(drawn[1].entry_fill_ms, None);
    assert!(drawn.iter().all(|o| o.closed_ms == Some(9_000.0)));
}

#[test]
fn other_markets_see_nothing_and_stop_markers_ride_the_line() {
    let mut with_stop = trace(true, ArchivedLineKind::Exit, &[(2_000.0, 1.5)]);
    with_stop.stop_price = Some(1.4);
    with_stop.stop_time_ms = Some(3_000.0);
    let store = OrderLineStore::archived(input(), &[with_stop]);
    assert!(store.market_draw_orders("BTCUSDT", usize::MAX).is_empty());
    let order = store.market_draw_orders("ADAUSDT", usize::MAX)[0];
    let line = &order.lines[LineKind::Sell as usize];
    assert_eq!(line.server_stop_price, Some(1.4));
    assert_eq!(line.server_stop_time_ms, Some(3_000.0));
}

#[test]
fn appended_neighbours_are_pale_and_keep_their_own_fill() {
    let mut store = OrderLineStore::archived(
        input(),
        &[trace(true, ArchivedLineKind::Entry, &[(1_000.0, 1.0)])],
    );
    let neighbour = ArchivedOrdersInput {
        market: "ADAUSDT",
        is_short: true,
        quantity: 5.0,
        entry_fill_ms: Some(20_500.0),
        close_ms: 30_000.0,
        bright: false,
    };
    store.append_archived(
        neighbour,
        &[
            trace(true, ArchivedLineKind::Entry, &[(20_000.0, 2.0)]),
            trace(false, ArchivedLineKind::Exit, &[(19_000.0, 2.5)]),
            trace(true, ArchivedLineKind::Entry, &[(21_000.0, 2.1)]),
        ],
    );
    let mut drawn = store.market_draw_orders("ADAUSDT", usize::MAX);
    drawn.sort_by_key(|o| o.uid);
    assert_eq!(drawn.len(), 4);
    assert!(drawn[0].subject);
    assert!(drawn[1..].iter().all(|o| !o.subject && o.is_short));
    assert_eq!(
        drawn[1].entry_fill_ms,
        Some(20_500.0),
        "first own entry ends at the fill"
    );
    assert_eq!(drawn[2].entry_fill_ms, None, "inherited: undated");
    assert_eq!(
        drawn[3].entry_fill_ms, None,
        "a second own entry: undated, as for the subject"
    );
    assert_eq!(drawn[1].closed_ms, Some(30_000.0));
    assert_eq!(
        drawn[0].closed_ms,
        Some(9_000.0),
        "the subject keeps its own close"
    );
}
