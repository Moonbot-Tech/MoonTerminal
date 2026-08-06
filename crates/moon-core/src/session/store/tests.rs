use std::net::{IpAddr, Ipv4Addr};

use super::{BalanceState, ConnStatus, CoreData};
use crate::feed::{ApiKeyExpiry, CoreEndpoint, CoreSysStatus, FeedMsg};

/// A core with the given freshness inputs; everything else stays at its default.
fn core(assets_rev: u64, rate_known: bool, stale: bool, status: ConnStatus) -> CoreData {
    let mut cd = CoreData::new();
    cd.assets_rev = assets_rev;
    cd.assets.global.usd_rate_known = rate_known;
    cd.assets_stale = stale;
    cd.status = status;
    cd
}

/// No snapshot yet is UNKNOWN, never zero — the distinction the Assets panel exists to make.
#[test]
fn without_a_snapshot_the_balance_is_awaiting() {
    let cd = core(0, true, false, ConnStatus::Ready);
    assert_eq!(cd.balance_state(), BalanceState::Awaiting);
    assert!(!cd.balance_state().has_value());
}

/// An unvaluable snapshot outranks staleness: there is no number, so its age is moot.
#[test]
fn unpriced_outranks_stale() {
    let cd = core(7, false, true, ConnStatus::Disconnected);
    assert_eq!(cd.balance_state(), BalanceState::Unpriced);
    assert!(!cd.balance_state().has_value());
}

/// A live connection is not enough on its own. `assets_rev` and the snapshot both survive a
/// reconnect, so a core back at `Ready` still carries the retained figure until a new snapshot
/// clears the marker — without this the pre-outage balance would be re-promoted to Live.
#[test]
fn a_reconnected_core_stays_stale_until_the_marker_clears() {
    let reconnected = core(7, true, true, ConnStatus::Ready);
    assert_eq!(reconnected.balance_state(), BalanceState::Stale);
    // A retained figure is still a figure: it is shown, but only with its stale marker.
    assert!(reconnected.balance_state().has_value());
    assert!(!reconnected.balance_state().is_current());
}

/// The other half of staleness: a snapshot that arrived before the link ever reached `Ready`.
#[test]
fn a_snapshot_from_a_not_ready_link_is_stale() {
    let cd = core(7, true, false, ConnStatus::Connecting);
    assert_eq!(cd.balance_state(), BalanceState::Stale);
}

/// Ready, priced, and with no stale marker — the only combination that renders at full
/// strength.
#[test]
fn ready_priced_and_unmarked_is_live() {
    let cd = core(7, true, false, ConnStatus::Ready);
    assert_eq!(cd.balance_state(), BalanceState::Live);
    assert!(cd.balance_state().has_value());
    assert!(cd.balance_state().is_current());
}

/// `code()` must separate every variant: it is hashed into a render signature, so a collision
/// would let one trust state be cached as another.
#[test]
fn every_state_hashes_distinctly() {
    let all = [
        BalanceState::Live,
        BalanceState::Stale,
        BalanceState::Awaiting,
        BalanceState::Unpriced,
    ];
    let mut codes: Vec<u64> = all.iter().map(|s| s.code()).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), all.len());
    // Only Live is current, and only Live/Stale carry a number.
    assert_eq!(all.iter().filter(|s| s.is_current()).count(), 1);
    assert_eq!(all.iter().filter(|s| s.has_value()).count(), 2);
}

/// `store.rs:CoreData::apply` must compare endpoint updates before bumping `sys_rev`; removing the
/// comparison churns Core Status on duplicate messages, while ignoring a changed endpoint leaves
/// the process displayed under the wrong server.
#[test]
fn an_endpoint_change_invalidates_core_status_once() {
    let first = CoreEndpoint {
        address: IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
        port: 3000,
    };
    let second = CoreEndpoint {
        address: IpAddr::V4(Ipv4Addr::new(192, 0, 2, 11)),
        port: 3000,
    };
    let mut core = CoreData::new();

    core.apply(FeedMsg::Endpoint(first));
    assert_eq!(core.endpoint, Some(first));
    assert_eq!(core.sys_rev, 1);

    core.apply(FeedMsg::Endpoint(first));
    assert_eq!(core.sys_rev, 1);

    core.apply(FeedMsg::Endpoint(second));
    assert_eq!(core.endpoint, Some(second));
    assert_eq!(core.sys_rev, 2);
}

/// `store.rs:CoreData::begin_connection_attempt` must clear both endpoint and telemetry; retaining
/// either moves the previous machine's CPU/RAM under a replacement key before fresh health arrives.
#[test]
fn a_replacement_feed_clears_endpoint_scoped_health() {
    let endpoint = CoreEndpoint {
        address: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 8)),
        port: 3000,
    };
    let health = CoreSysStatus {
        process_cpu_percent: Some(21),
        system_cpu_percent: Some(44),
        used_memory_mb: Some(512),
        free_physical_memory_mb: Some(4096),
        logical_cpu_count: Some(16),
        round_trip_ms: Some(180),
        order_api_latency_ms: Some(60),
        updated_ms: 123,
    };
    let mut core = CoreData::new();
    core.apply(FeedMsg::Endpoint(endpoint));
    core.apply(FeedMsg::SysStatus(health));
    let previous_rev = core.sys_rev;

    core.begin_connection_attempt();

    assert_eq!(core.status, ConnStatus::Connecting);
    assert_eq!(core.endpoint, None);
    assert_eq!(core.sys, CoreSysStatus::default());
    assert_eq!(core.sys_rev, previous_rev + 1);
}

/// The API-key poll re-reports the same answer every few hours, and MoonProto rebuilds the absolute
/// date from the CURRENT clock each time, so a byte-equal answer never arrives. The revision must
/// track what the answer SAYS — anything watching it would otherwise see a change every six hours
/// on a key that did not move.
#[test]
fn an_unchanged_key_answer_does_not_bump_the_revision() {
    let mut core = CoreData::new();
    let first = ApiKeyExpiry {
        unlimited: false,
        known: true,
        days_left: Some(30),
        at_unix: Some(1_800_000_000),
        checked_ms: 1_000,
    };
    core.apply(FeedMsg::ApiExpiry(first));
    let after_first = core.api_expiry_rev;

    core.apply(FeedMsg::ApiExpiry(ApiKeyExpiry {
        checked_ms: 1_000 + 6 * 60 * 60 * 1_000,
        ..first
    }));

    assert_eq!(core.api_expiry_rev, after_first, "same answer, later check");
    assert_eq!(
        core.api_expiry.map(|e| e.checked_ms),
        Some(1_000 + 6 * 60 * 60 * 1_000),
        "the newer receipt time is still retained"
    );
}

/// A key that lost a day is a real change and must reach the panel.
#[test]
fn a_changed_day_count_bumps_the_revision() {
    let mut core = CoreData::new();
    let first = ApiKeyExpiry {
        unlimited: false,
        known: true,
        days_left: Some(8),
        at_unix: Some(1_800_000_000),
        checked_ms: 1_000,
    };
    core.apply(FeedMsg::ApiExpiry(first));
    let after_first = core.api_expiry_rev;

    core.apply(FeedMsg::ApiExpiry(ApiKeyExpiry {
        days_left: Some(7),
        ..first
    }));

    assert_eq!(core.api_expiry_rev, after_first + 1);
}
