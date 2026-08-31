//! Mutation proofs for the per-IP core-update queue.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use super::*;
use crate::feed::{ConnStatus, CoreEndpoint, FeedMsg, UpdateTarget};
use crate::market::{MarketDataMode, MarketDataSource, MarketStore};
use crate::session::CoreStore;
use crate::session::store::CoreData;

/// Build a manager with no feed threads, so queue transitions can be driven by retained store data.
fn manager() -> SessionManager {
    let market = MarketStore::shared(0.0);
    let market_source = MarketDataSource::new(market.clone());
    SessionManager {
        sessions: Vec::new(),
        config_order: Vec::new(),
        feed_wake: None,
        store: CoreStore::default(),
        market,
        market_source,
        mode: MarketDataMode::default(),
        core_venue: HashMap::new(),
        core_base: HashMap::new(),
        core_provider: HashMap::new(),
        providers: HashMap::new(),
        wanted: HashMap::new(),
        pending_drop: HashMap::new(),
        last_cmd: HashMap::new(),
        core_updates: CoreUpdateQueue::default(),
    }
}

/// Return a ready core snapshot at `address`, with the update-completion inputs populated.
fn ready_core(address: IpAddr) -> CoreData {
    let mut data = CoreData::new();
    data.status = ConnStatus::Ready;
    data.endpoint = Some(CoreEndpoint {
        address,
        port: 8_888,
    });
    data.server_version = Some(100);
    data.startup.state = crate::feed::CoreStartupState::Ready;
    data
}

/// Insert one retained core snapshot into a test manager without opening a live feed.
fn insert_core(manager: &mut SessionManager, core: CoreId, data: CoreData) {
    manager.store.ensure(core);
    *manager
        .store
        .core_mut(core)
        .expect("ensure must create the requested core") = data;
}

/// `core_update.rs:pop_ready_lanes` must defer a ready sibling when another in-flight core moved
/// onto its IP; deleting the cross-lane scan would start two simultaneous MoonBot updates there.
#[test]
fn moved_in_flight_core_blocks_a_ready_sibling_on_its_new_ip() {
    let old_ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
    let shared_ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
    let moved_core = 1;
    let queued_core = 2;
    let mut manager = manager();

    insert_core(&mut manager, moved_core, ready_core(shared_ip));
    insert_core(&mut manager, queued_core, ready_core(shared_ip));
    manager.core_updates.phases.insert(
        moved_core,
        CoreUpdatePhase::Sent {
            target: UpdateTarget::Release,
            from: Some(100),
            epoch0: 0,
            sent_at_ms: 0,
        },
    );
    manager.core_updates.lanes.insert(
        old_ip,
        Lane {
            order: VecDeque::new(),
            active: Some(moved_core),
            stalled: false,
        },
    );
    manager.core_updates.phases.insert(
        queued_core,
        CoreUpdatePhase::Queued {
            lane: shared_ip,
            held: false,
            not_ready_since: None,
        },
    );
    manager.core_updates.attempts.insert(
        queued_core,
        AttemptMeta {
            started_ms: 0,
            core_name: "queued".to_string(),
            target: UpdateTarget::Release,
            from: Some(100),
        },
    );
    manager.core_updates.lanes.insert(
        shared_ip,
        Lane {
            order: VecDeque::from([queued_core]),
            active: None,
            stalled: false,
        },
    );

    manager.pop_ready_lanes(1);

    assert!(
        matches!(
            manager.core_update_phase(queued_core),
            Some(CoreUpdatePhase::Queued { lane, .. }) if *lane == shared_ip
        ),
        "a live core that moved onto this IP must defer the sibling instead of starting it"
    );
}

/// `core_update.rs:advance_in_flight_updates` must require `conn_epoch > epoch0`; changing it to
/// `>=` accepts a core that never departed and lets its IP lane advance while the update runs.
#[test]
fn unchanged_connection_epoch_keeps_a_sent_update_out_of_waiting() {
    let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 1, 1));
    let core = 7;
    let mut manager = manager();
    let mut data = ready_core(ip);
    data.conn_epoch = 41;
    insert_core(&mut manager, core, data);
    manager.core_updates.phases.insert(
        core,
        CoreUpdatePhase::Sent {
            target: UpdateTarget::Release,
            from: Some(100),
            epoch0: 41,
            sent_at_ms: 0,
        },
    );
    manager.core_updates.lanes.insert(
        ip,
        Lane {
            order: VecDeque::new(),
            active: Some(core),
            stalled: false,
        },
    );

    manager.advance_in_flight_updates(1);

    assert!(
        matches!(
            manager.core_update_phase(core),
            Some(CoreUpdatePhase::Sent { epoch0: 41, .. })
        ),
        "unchanged epoch proves the core never left Ready, so this update cannot enter Waiting"
    );
}

/// `core_update.rs:advance_in_flight_updates` must require `conn_epoch > epoch1`; changing it
/// to `>=` accepts the pre-respawn build and leaves the terminal reporting a stale MoonBot version.
#[test]
fn verifying_waits_for_a_fresh_connection_before_accepting_a_build() {
    let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 1, 2));
    let core = 8;
    let epoch1 = 41;
    let mut manager = manager();
    let mut data = ready_core(ip);
    data.conn_epoch = epoch1;
    insert_core(&mut manager, core, data);
    manager.core_updates.phases.insert(
        core,
        CoreUpdatePhase::Verifying {
            target: UpdateTarget::Release,
            from: Some(100),
            epoch1,
            sent_at_ms: 0,
            left_at_ms: 1,
            verify_at_ms: 1,
        },
    );
    manager.core_updates.lanes.insert(
        ip,
        Lane {
            order: VecDeque::new(),
            active: Some(core),
            stalled: false,
        },
    );

    manager.advance_in_flight_updates(2);

    assert!(
        matches!(
            manager.core_update_phase(core),
            Some(CoreUpdatePhase::Verifying { epoch1: 41, .. })
        ),
        "the pre-respawn snapshot must not complete verification before a fresh client connects"
    );

    manager
        .store
        .core_mut(core)
        .expect("inserted core must remain in the retained store")
        .begin_connection_attempt();
    manager.advance_in_flight_updates(3);

    assert!(
        matches!(
            manager.core_update_phase(core),
            Some(CoreUpdatePhase::Verifying { .. })
        ),
        "a respawning core cannot verify while its old snapshot has been cleared"
    );

    let data = manager
        .store
        .core_mut(core)
        .expect("inserted core must remain in the retained store");
    data.apply(FeedMsg::Status(ConnStatus::Ready));
    data.startup.state = crate::feed::CoreStartupState::Ready;
    manager.advance_in_flight_updates(4);

    assert!(
        matches!(
            manager.core_update_phase(core),
            Some(CoreUpdatePhase::Verifying { .. })
        ),
        "a new epoch alone cannot verify the update until the fresh client reports its build"
    );

    manager
        .store
        .core_mut(core)
        .expect("inserted core must remain in the retained store")
        .server_version = Some(101);
    manager.advance_in_flight_updates(5);

    assert!(
        matches!(
            manager.core_update_phase(core),
            Some(CoreUpdatePhase::Done(CoreUpdateOutcome::Succeeded {
                from: Some(100),
                to: 101
            }))
        ),
        "the fresh connection and fresh build must settle verification as the observed update"
    );
}

/// `core_update.rs:active_from` must keep `Verifying` active; moving it to `None` starts a
/// sibling update on the same IP while the first MoonBot update is still being verified.
#[test]
fn verifying_core_blocks_a_ready_sibling_on_its_new_ip() {
    let old_ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3));
    let shared_ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 4));
    let verifying_core = 3;
    let queued_core = 4;
    let mut manager = manager();

    insert_core(&mut manager, verifying_core, ready_core(shared_ip));
    insert_core(&mut manager, queued_core, ready_core(shared_ip));
    manager.core_updates.phases.insert(
        verifying_core,
        CoreUpdatePhase::Verifying {
            target: UpdateTarget::Release,
            from: Some(100),
            epoch1: 12,
            sent_at_ms: 0,
            left_at_ms: 1,
            verify_at_ms: 1,
        },
    );
    manager.core_updates.lanes.insert(
        old_ip,
        Lane {
            order: VecDeque::new(),
            active: Some(verifying_core),
            stalled: false,
        },
    );
    manager.core_updates.phases.insert(
        queued_core,
        CoreUpdatePhase::Queued {
            lane: shared_ip,
            held: false,
            not_ready_since: None,
        },
    );
    manager.core_updates.attempts.insert(
        queued_core,
        AttemptMeta {
            started_ms: 0,
            core_name: "queued".to_string(),
            target: UpdateTarget::Release,
            from: Some(100),
        },
    );
    manager.core_updates.lanes.insert(
        shared_ip,
        Lane {
            order: VecDeque::from([queued_core]),
            active: None,
            stalled: false,
        },
    );

    manager.pop_ready_lanes(2);

    assert!(
        matches!(
            manager.core_update_phase(queued_core),
            Some(CoreUpdatePhase::Queued { lane, .. }) if *lane == shared_ip
        ),
        "a verifying core that moved onto this IP must defer the sibling instead of starting it"
    );
}

/// `store.rs:CoreData::apply` must bump `conn_epoch` only on `Ready -> not-Ready`; changing the
/// edge guard to `true` counts reconnect backoff messages as departures and falsely completes an update.
#[test]
fn reconnect_backoff_counts_as_one_connection_departure() {
    let mut data = CoreData::new();

    data.apply(FeedMsg::Status(ConnStatus::Ready));
    data.apply(FeedMsg::Status(ConnStatus::Stage("reconnecting".into())));
    data.apply(FeedMsg::Status(ConnStatus::Connecting));

    assert_eq!(
        data.conn_epoch, 1,
        "one Ready-to-down transition is one departure even when reconnect backoff reports twice"
    );
}
