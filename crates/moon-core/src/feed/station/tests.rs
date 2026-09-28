use super::*;
use moonproto::state::{BalanceEvent, OrderEvent};

/// The station keeps nothing of the account: a widened filter would put the balance-repair and
/// Assets paths — and their requests to the core — back on a host that stores neither.
#[test]
fn account_and_order_events_do_not_reach_the_station() {
    let balance = Event::Balance(BalanceEvent::IncrementalApplied {
        count: 1,
        global_changed: true,
    });
    assert!(!keeps(&balance));
    assert!(!keeps(&Event::Order(OrderEvent::Snapshot)));
}
