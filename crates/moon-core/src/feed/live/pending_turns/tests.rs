//! Folding captured turns: what a long stall may drop and what it must keep.

use super::{FOLD_ROWS, PendingTurns};
use crate::feed::tests::order_queue::row;
use crate::feed::{FeedMsg, OrderRow};

/// A row of order `uid` whose sell price tells its captures apart.
fn priced(uid: u64, sell_price: f64) -> OrderRow {
    let mut row = row(uid);
    row.sell_price = sell_price;
    row
}

/// `(uid, sell price)` of every row, turn by turn.
fn shape(turns: &[Vec<OrderRow>]) -> Vec<Vec<(u64, f64)>> {
    turns
        .iter()
        .map(|turn| turn.iter().map(|row| (row.uid, row.sell_price)).collect())
        .collect()
}

/// The fold is for a stalled consumer only. A feed whose publications follow each other must
/// hand over every turn as it was captured: the intermediate reprices are what the chart draws.
#[test]
fn turns_below_the_fold_are_handed_over_as_captured() {
    let mut pending = PendingTurns::default();
    pending.push(vec![priced(1, 10.0), priced(2, 20.0)]);
    pending.push(vec![priced(1, 11.0), priced(1, 12.0)]);
    pending.push(Vec::new());

    let turns = pending.take();

    assert_eq!(
        shape(&turns),
        [
            vec![(1, 10.0), (2, 20.0)],
            vec![(1, 11.0), (1, 12.0)],
            vec![]
        ]
    );
    assert!(pending.take().is_empty(), "taken turns were kept");
}

/// #1081: while nothing takes the queued table the feed thread publishes nothing, and the turns
/// it keeps capturing must not become the next thing that grows with the length of the stall.
/// Forty orders repriced twenty thousand times stay bounded, and each ends on its last price.
#[test]
fn a_long_stall_keeps_the_latest_row_of_each_order() {
    let mut pending = PendingTurns::default();
    let mut last_price = std::collections::HashMap::new();
    for turn in 0..20_000u64 {
        let uid = turn % 40;
        let price = turn as f64;
        last_price.insert(uid, price);
        pending.push(vec![priced(uid, price)]);
    }

    let turns = pending.take();

    let rows: usize = turns.iter().map(Vec::len).sum();
    assert!(
        rows <= FOLD_ROWS + 1,
        "{rows} rows kept for forty orders: the turns grow with the stall"
    );
    // Replaying in order, the last row of an order is the state the line store ends on.
    let mut replayed = std::collections::HashMap::new();
    for row in turns.iter().flatten() {
        replayed.insert(row.uid, row.sell_price);
    }
    assert_eq!(replayed, last_price);
}

/// The publication overlays the LAST turn on its snapshot, and an empty last turn means "a
/// resynchronised snapshot followed, overlay nothing". Merging the older rows into it would lay
/// rows from before the resynchronisation over the snapshot that replaced them.
#[test]
fn the_last_turn_is_never_merged_into() {
    let mut pending = PendingTurns::default();
    let earlier: Vec<_> = (0..FOLD_ROWS as u64).map(|uid| priced(uid, 1.0)).collect();
    let latest: Vec<_> = (0..FOLD_ROWS as u64).map(|uid| priced(uid, 2.0)).collect();
    pending.push(earlier);
    pending.push(latest);
    pending.push(Vec::new());

    let turns = pending.take();

    assert_eq!(turns.len(), 3);
    assert!(turns[0].iter().all(|row| row.sell_price == 1.0));
    assert!(turns[1].iter().all(|row| row.sell_price == 2.0));
    assert!(turns[2].is_empty(), "the snapshot-only marker was lost");
}

/// A core with more open orders than the fold allows would otherwise fold on every push and
/// walk its whole set each time. After a fold the next one waits for the set to double.
#[test]
fn a_set_above_the_fold_does_not_fold_again_on_every_push() {
    let mut pending = PendingTurns::default();
    let many = FOLD_ROWS as u64 * 4;
    pending.push((0..many).map(|uid| priced(uid, 1.0)).collect());
    for uid in many..many + 100 {
        pending.push(vec![priced(uid, 1.0)]);
    }

    assert_eq!(
        pending.take().len(),
        101,
        "the turns were folded again while the set had barely grown"
    );
}

/// Trace points, corridor changes and resynchronisations are order events that carry no row, so
/// each leaves an empty turn. They keep coming while the consumer is stalled, and a fold that
/// counted rows alone would never see them: the list of turns would grow with the stall.
#[test]
fn rowless_turns_do_not_pile_up_through_a_stall() {
    let mut pending = PendingTurns::default();
    pending.push(vec![priced(7, 1.0)]);
    for _ in 0..20_000 {
        pending.push(Vec::new());
    }

    let turns = pending.take();

    assert!(
        turns.len() <= FOLD_ROWS + 2,
        "{} turns kept: the rowless ones grow with the stall",
        turns.len()
    );
    assert_eq!(shape(&turns)[0], [(7, 1.0)], "the captured row was lost");
    assert!(
        turns.last().is_some_and(Vec::is_empty),
        "the snapshot-only marker was lost"
    );
}

/// What the fold exists to preserve: an order that closed during the stall and is gone from
/// the snapshot. Its terminal row must still reach the line store through the folded turns,
/// exactly as it would have through the turns as captured.
#[test]
fn a_folded_stall_closes_the_same_orders_as_the_turns_it_replaced() {
    use crate::feed::live::coalesced_order_rows;
    use crate::session::store::CoreData;

    let mut closed = priced(1, 30.0);
    closed.job_is_done = true;
    closed.status = "Done".into();
    let mut captured = vec![vec![priced(1, 10.0)], vec![priced(1, 20.0)], vec![closed]];
    // Enough other orders moving afterwards to fold the turns above more than once.
    for turn in 0..(FOLD_ROWS as u64 * 3) {
        captured.push(vec![priced(100 + turn % 50, turn as f64)]);
    }
    let mut pending = PendingTurns::default();
    for turn in captured.clone() {
        pending.push(turn);
    }
    let folded = pending.take();
    assert!(
        folded.len() < captured.len() / 2,
        "the fixture never folded"
    );
    // The snapshot the stall ends on no longer lists order 1.
    let snapshot: Vec<_> = (100..150).map(|uid| priced(uid, 99.0)).collect();

    let mut reference = CoreData::new();
    reference.apply(FeedMsg::Orders(coalesced_order_rows(
        snapshot.clone(),
        captured,
    )));
    let mut actual = CoreData::new();
    actual.apply(FeedMsg::Orders(coalesced_order_rows(snapshot, folded)));

    let closed_in = |core: &CoreData| core.order_lines.order_state(1).map(|state| state.active);
    assert_eq!(closed_in(&reference), Some(false), "the fixture is wrong");
    assert_eq!(closed_in(&actual), Some(false));
    for uid in 100..150 {
        assert_eq!(
            actual
                .order_lines
                .order_state(uid)
                .map(|state| state.active),
            reference
                .order_lines
                .order_state(uid)
                .map(|state| state.active)
        );
    }
    assert_eq!(
        format!("{:?}", actual.orders),
        format!("{:?}", reference.orders)
    );
}
