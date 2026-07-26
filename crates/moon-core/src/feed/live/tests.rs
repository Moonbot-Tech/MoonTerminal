use super::*;
use moonproto::state::BalanceEvent;

/// `live/mod.rs:should_publish_assets` removing the Balance-event bypass would leave the header's
/// free funds stale until an unrelated event arrives after the five-second background interval.
#[test]
fn balance_events_bypass_the_background_assets_throttle() {
    let balance = Event::Balance(BalanceEvent::IncrementalApplied {
        count: 1,
        global_changed: true,
    });
    let presentation_only = Event::Order(OrderEvent::Snapshot);

    assert!(should_publish_assets(
        &[balance],
        Duration::from_secs(1),
        Duration::from_secs(5)
    ));
    assert!(!should_publish_assets(
        &[presentation_only],
        Duration::from_secs(1),
        Duration::from_secs(5)
    ));
}
