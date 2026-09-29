//! A core that restarts or switches exchange gets its new venue through a respawn (#734).
//!
//! Driven through the production drain with synthetic feed messages; the respawn itself is
//! represented by `clear_core_coordination`, the part of `respawn_session` that drops what the
//! old client published.

use std::time::{Duration, Instant};

use super::trade_sound_tests::fixture;
use crate::feed::{ExchangeId, FeedMsg, FeedTx, IdentityStaleCause};
use crate::session::identity_respawn::MIN_RESPAWN_GAP;

/// Publish `code` as the core's venue, the way a fresh client does after BaseCheck.
fn identify(tx: &FeedTx, code: u8) {
    tx.send(FeedMsg::Identity {
        id: ExchangeId::new(code),
        dex: String::new(),
        reported: String::new(),
    })
    .unwrap();
}

fn venue_code(session: &super::SessionManager) -> Option<u8> {
    session.core_venues().get(&1).map(|venue| venue.id.code)
}

#[test]
fn server_restart_respawns_and_publishes_the_new_venue() {
    let (mut session, tx) = fixture();
    identify(&tx, 1);
    session.drain();
    assert_eq!(venue_code(&session), Some(1));

    tx.send(FeedMsg::IdentityStale(IdentityStaleCause::ServerRestart))
        .unwrap();
    session.drain();
    let now = Instant::now();
    assert_eq!(session.take_identity_respawn_requests(now), vec![1]);
    assert!(session.take_identity_respawn_requests(now).is_empty());

    // The respawn drops the old client's venue before the fresh one names the new exchange.
    session.clear_core_coordination(1);
    assert_eq!(
        venue_code(&session),
        None,
        "the stale venue must not survive"
    );
    identify(&tx, 2);
    session.drain();
    assert_eq!(venue_code(&session), Some(2));
}

#[test]
fn hot_switch_turnover_requests_a_respawn() {
    let (mut session, tx) = fixture();
    identify(&tx, 1);
    tx.send(FeedMsg::IdentityStale(IdentityStaleCause::MarketTurnover {
        added: 200,
        total: 600,
    }))
    .unwrap();
    session.drain();
    assert_eq!(
        session.take_identity_respawn_requests(Instant::now()),
        vec![1]
    );
}

/// A flapping core respawns at most once per gap, and the restart inside the gap is deferred,
/// not lost.
#[test]
fn a_flapping_core_is_debounced_but_not_dropped() {
    let (mut session, tx) = fixture();
    let t0 = Instant::now();
    tx.send(FeedMsg::IdentityStale(IdentityStaleCause::ServerRestart))
        .unwrap();
    session.drain();
    assert_eq!(session.take_identity_respawn_requests(t0), vec![1]);

    for _ in 0..5 {
        tx.send(FeedMsg::IdentityStale(IdentityStaleCause::ServerRestart))
            .unwrap();
    }
    session.drain();
    let inside = t0 + MIN_RESPAWN_GAP - Duration::from_millis(1);
    assert!(session.take_identity_respawn_requests(inside).is_empty());
    assert_eq!(
        session.take_identity_respawn_requests(t0 + MIN_RESPAWN_GAP),
        vec![1],
        "five restarts inside the gap collapse into one deferred respawn"
    );
    assert!(
        session
            .take_identity_respawn_requests(t0 + MIN_RESPAWN_GAP * 3)
            .is_empty()
    );
}

/// A removed core leaves nothing pending that a later id reuse would inherit.
#[test]
fn a_dropped_core_forgets_its_pending_request() {
    let (mut session, tx) = fixture();
    tx.send(FeedMsg::IdentityStale(IdentityStaleCause::ServerRestart))
        .unwrap();
    session.drain();
    session.drop_core(1);
    assert!(
        session
            .take_identity_respawn_requests(Instant::now())
            .is_empty()
    );
}

#[test]
fn a_respawn_from_elsewhere_serves_the_deferred_request() {
    use crate::session::identity_respawn::IdentityRespawnGate;
    let core = 7;
    let t0 = Instant::now();
    let mut gate = IdentityRespawnGate::default();
    gate.request(core);
    assert_eq!(gate.take_due(t0), vec![core]);
    gate.request(core);
    gate.fulfilled(core);
    let later = t0 + MIN_RESPAWN_GAP;
    assert!(gate.take_due(later).is_empty());
}
