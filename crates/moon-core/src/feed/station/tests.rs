use super::*;
use moonproto::state::{BalanceEvent, OrderEvent};

/// The light station keeps nothing of the account: a widened filter would put the balance-repair
/// and Assets paths — and their requests to the core — back on a host that stores neither.
#[test]
fn account_and_order_events_do_not_reach_the_light_station() {
    let balance = Event::Balance(BalanceEvent::IncrementalApplied {
        count: 1,
        global_changed: true,
    });
    assert!(!Profile::Reports.keeps(&balance));
    assert!(!Profile::Reports.keeps(&Event::Order(OrderEvent::Snapshot)));
}

/// The Mini App's station keeps what its tabs read: without orders the open-orders tab and Panic
/// Sell's state stay empty, without balances the Cores tab.
#[test]
fn the_mini_app_station_keeps_the_account() {
    let balance = Event::Balance(BalanceEvent::IncrementalApplied {
        count: 1,
        global_changed: true,
    });
    assert!(Profile::Account.keeps(&balance));
    assert!(Profile::Account.keeps(&Event::Order(OrderEvent::Snapshot)));
}
