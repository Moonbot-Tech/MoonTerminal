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
        exit: None,
        bright: false,
    }
}

/// The exit the report states for the fixture trade: placed at 3_000 at 1.7, filled at the close.
fn report_exit() -> ReportExit {
    ReportExit {
        price: 1.7,
        set_ms: 3_000.0,
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
        exit: None,
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

/// The core archives a line only once its chart gave it a point, so a trade that closed within a
/// second answers with nothing — and the terminal still knows where its exit stood: the report's
/// exit price from the exit order's creation to the close.
#[test]
fn an_empty_archive_still_draws_the_reports_exit_line() {
    let store = OrderLineStore::archived(
        ArchivedOrdersInput {
            exit: Some(report_exit()),
            ..input()
        },
        &[],
    );
    let drawn = store.market_draw_orders("ADAUSDT", usize::MAX);
    assert_eq!(drawn.len(), 1);
    let order = drawn[0];
    assert!(order.subject);
    assert_eq!(order.create_ms, 3_000.0);
    assert_eq!(order.closed_ms, Some(9_000.0));
    assert!(order.lines[LineKind::Buy as usize].steps.is_empty());
    assert_eq!(
        order.lines[LineKind::Sell as usize].steps,
        vec![(3_000.0, 1.7)]
    );
    assert!(
        order.lines[LineKind::Sell as usize]
            .server_points
            .is_empty(),
        "a line the report placed has no repricing path to draw"
    );
}

/// A market entry typically answers with the exit line alone — and the reverse, an archived entry
/// with no exit, joins the report's exit onto the same subject order.
#[test]
fn an_archived_entry_without_an_exit_gets_the_reports_exit_beside_it() {
    let store = OrderLineStore::archived(
        ArchivedOrdersInput {
            exit: Some(report_exit()),
            ..input()
        },
        &[trace(true, ArchivedLineKind::Entry, &[(1_000.0, 1.0)])],
    );
    let drawn = store.market_draw_orders("ADAUSDT", usize::MAX);
    assert_eq!(drawn.len(), 1, "one subject order carries both lines");
    let order = drawn[0];
    assert_eq!(
        order.create_ms, 1_000.0,
        "the entry's start stays the order's start"
    );
    assert_eq!(order.entry_fill_ms, Some(1_500.0));
    assert_eq!(
        order.lines[LineKind::Buy as usize].steps,
        vec![(1_000.0, 1.0)]
    );
    assert_eq!(
        order.lines[LineKind::Sell as usize].steps,
        vec![(3_000.0, 1.7)]
    );
}

/// An own exit line in the archive wins over the report's: it carries the repricing path the
/// report cannot state. An INHERITED exit does not count — it is an ancestor's.
#[test]
fn an_archived_own_exit_line_replaces_the_reports() {
    let store = OrderLineStore::archived(
        ArchivedOrdersInput {
            exit: Some(report_exit()),
            ..input()
        },
        &[trace(
            true,
            ArchivedLineKind::Exit,
            &[(2_000.0, 1.5), (2_500.0, 1.6)],
        )],
    );
    let drawn = store.market_draw_orders("ADAUSDT", usize::MAX);
    assert_eq!(drawn.len(), 1);
    assert_eq!(
        drawn[0].lines[LineKind::Sell as usize].steps,
        vec![(2_000.0, 1.6)]
    );
    assert_eq!(
        drawn[0].lines[LineKind::Sell as usize].server_points.len(),
        2
    );

    let mut store = OrderLineStore::archived(input(), &[]);
    store.append_archived(
        ArchivedOrdersInput {
            exit: Some(report_exit()),
            ..input()
        },
        &[trace(false, ArchivedLineKind::Exit, &[(500.0, 0.9)])],
    );
    let mut drawn = store.market_draw_orders("ADAUSDT", usize::MAX);
    drawn.sort_by_key(|o| o.uid);
    assert_eq!(
        drawn.len(),
        2,
        "the inherited exit and the report's own, as two orders"
    );
    assert!(
        drawn.iter().all(|o| !o.subject),
        "appended lines follow `bright`, the report's exit included"
    );
    assert_eq!(
        drawn[0].lines[LineKind::Sell as usize].steps,
        vec![(500.0, 0.9)]
    );
    assert_eq!(
        drawn[1].lines[LineKind::Sell as usize].steps,
        vec![(3_000.0, 1.7)]
    );
    assert_eq!(drawn[1].closed_ms, Some(9_000.0));
}

/// A seconds-only placement stamp raised past a millisecond close must not run backwards.
#[test]
fn a_placement_after_the_close_collapses_to_the_close() {
    let store = OrderLineStore::archived(
        ArchivedOrdersInput {
            exit: Some(ReportExit {
                price: 1.7,
                set_ms: 9_400.0,
            }),
            ..input()
        },
        &[],
    );
    let drawn = store.market_draw_orders("ADAUSDT", usize::MAX);
    assert_eq!(drawn[0].create_ms, 9_000.0);
    assert_eq!(
        drawn[0].lines[LineKind::Sell as usize].steps,
        vec![(9_000.0, 1.7)]
    );
}
