use super::*;

fn added(n: usize) -> MarketsEvent {
    MarketsEvent::NewMarketsAdded {
        names: (0..n).map(|i| format!("M{i}USDT")).collect(),
    }
}

#[test]
fn server_restart_is_stale_and_other_lifecycle_events_are_not() {
    assert_eq!(
        stale_on_lifecycle(&LifecycleEvent::ServerRestart),
        Some(IdentityStaleCause::ServerRestart)
    );
    for ev in [
        LifecycleEvent::Reconnecting,
        LifecycleEvent::Connected { fresh: false },
        LifecycleEvent::Ready,
        LifecycleEvent::Disconnected,
    ] {
        assert_eq!(stale_on_lifecycle(&ev), None, "{ev:?}");
    }
}

/// A new venue merged beside the old one adds half the retained list in one refresh.
#[test]
fn a_venue_sized_refresh_is_a_turnover() {
    assert_eq!(
        stale_on_markets(&added(200), || 600),
        Some(IdentityStaleCause::MarketTurnover {
            added: 200,
            total: 600
        })
    );
    assert!(stale_on_markets(&added(100), || 400).is_some());
}

#[test]
fn a_routine_listing_is_not_a_turnover() {
    assert_eq!(stale_on_markets(&added(3), || 400), None);
    assert_eq!(stale_on_markets(&added(99), || 400), None);
}

/// On a tiny core every listing is a large share; the absolute floor keeps it from counting.
#[test]
fn a_small_core_listing_is_not_a_turnover() {
    assert_eq!(
        stale_on_markets(&added(19), || panic!("total is not read below the floor")),
        None
    );
}

#[test]
fn a_full_list_refresh_is_not_a_turnover() {
    let ev = MarketsEvent::MarketsListReplaced {
        count: 600,
        corr_count: 0,
    };
    assert_eq!(stale_on_markets(&ev, || 600), None);
}
