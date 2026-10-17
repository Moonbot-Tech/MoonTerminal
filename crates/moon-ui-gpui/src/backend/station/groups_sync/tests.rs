//! Automatic groups must neither wipe another terminal's groups nor repeatedly retry failures.

use super::{GroupsSync, same_groups};
use crate::backend::station::{cores_sync::LocalCore, job::Job};
use moon_core::{
    config::CoreGroup,
    station_api::{Access, ListedCore},
};
use moon_remote::ssh::Target;

/// One independently numbered station identity matching the local endpoint and fingerprint.
fn fixture() -> (Vec<CoreGroup>, Vec<LocalCore>, Vec<ListedCore>, Access) {
    let groups = vec![CoreGroup {
        name: "GROUP_A".into(),
        cores: vec![1],
    }];
    let here = vec![LocalCore {
        uid: 1,
        name: "CORE_A".into(),
        address: Some("core-a.example.invalid:4510".into()),
        key_address: Some("core-a.example.invalid:4510".into()),
        endpoint_override: String::new(),
        key_fp: "KEY_A".into(),
    }];
    let listing = vec![ListedCore {
        uid: 11,
        name: "CORE_A".into(),
        address: here[0].address.clone(),
        key_fp: Some("KEY_A".into()),
        endpoint_override: Some(String::new()),
    }];
    let access = Access {
        groups: Some(vec![]),
        ..Default::default()
    };
    (groups, here, listing, access)
}

/// Build a synthetic destination without touching the saved hosts or network.
fn target() -> Option<Target> {
    Some(Target {
        host: "station.example.invalid".into(),
        port: 22,
    })
}

/// Removing the admission latch would loop on failed or rejected writes, even after rereads.
#[test]
fn groups_sync_attempts_once_per_destination_and_changed_payload() {
    let (mut groups, here, listing, access) = fixture();
    let mut sync = GroupsSync::default();
    assert!(
        matches!(sync.take_job(false, target(), Some(&access), &groups, &here, &listing), Some(Job::Groups { groups: sent, .. }) if sent == groups)
    );
    // No acknowledgement: this models a failed write followed by an unrelated station read.
    assert!(
        sync.take_job(false, target(), Some(&access), &groups, &here, &listing)
            .is_none()
    );
    groups[0].name = "GROUP_B".into();
    assert!(
        sync.take_job(false, target(), Some(&access), &groups, &here, &listing)
            .is_some()
    );
    groups[0].name = "GROUP_A".into();
    assert!(
        sync.take_job(false, target(), Some(&access), &groups, &here, &listing)
            .is_none()
    );
    let other = Some(Target {
        host: "other.example.invalid".into(),
        port: 22,
    });
    assert!(
        sync.take_job(false, other, Some(&access), &groups, &here, &listing)
            .is_some()
    );
}

/// Busy and unmapped skips must not latch a payload that becomes safe after the queue drains.
#[test]
fn groups_sync_defers_busy_and_unmapped_without_latching() {
    let (groups, here, listing, access) = fixture();
    let mut sync = GroupsSync::default();
    assert!(
        sync.take_job(true, target(), Some(&access), &groups, &here, &listing)
            .is_none()
    );
    assert!(
        sync.take_job(false, target(), Some(&access), &groups, &here, &[])
            .is_none()
    );
    assert!(
        sync.take_job(false, target(), Some(&access), &groups, &here, &listing)
            .is_some()
    );
}

/// A fresh-listing skip must admit the same payload when mapping returns, without a tight retry.
#[test]
fn groups_sync_fresh_unmapped_deferral_releases_only_its_attempt() {
    let (groups, here, listing, access) = fixture();
    let mut sync = GroupsSync::default();
    assert!(
        sync.take_job(false, target(), Some(&access), &groups, &here, &listing)
            .is_some()
    );
    sync.translation_seen(1);
    assert!(
        sync.take_job(false, target(), Some(&access), &groups, &here, &[])
            .is_none()
    );
    assert!(
        sync.take_job(false, target(), Some(&access), &groups, &here, &listing)
            .is_some()
    );
    sync.translation_seen(0);
    assert!(
        sync.take_job(false, target(), Some(&access), &groups, &here, &listing)
            .is_none()
    );
}

/// Reconfiguring the same address represents a new station lifetime, not a retry of an old one.
#[test]
fn groups_sync_forgetting_station_clears_attempts_at_the_same_address() {
    let (groups, here, listing, access) = fixture();
    let mut station = super::super::StationJobs::default();
    assert!(
        station
            .groups_sync
            .take_job(false, target(), Some(&access), &groups, &here, &listing)
            .is_some()
    );
    station.clear_known(Ok(String::new()));
    assert!(
        station
            .groups_sync
            .take_job(false, target(), Some(&access), &groups, &here, &listing)
            .is_some()
    );
}

/// A restart gap retains the UI's listing but must not re-admit an unmapped automatic payload.
#[test]
fn groups_sync_deferral_waits_for_a_fresh_listing_despite_retained_core_cache() {
    use moon_remote::station::bot::BotState;
    let (groups, here, listing, access) = fixture();
    let status = moon_core::station_api::Status {
        station_version: "synthetic".into(),
        core_uid_high_water: Some(11),
        cores_total: 1,
        cores_ready: 1,
        cores: Some(listing.clone()),
        bot: None,
        tape: None,
        host: None,
        tape_catchup: None,
        last_update: None,
        auto_update: None,
    };
    let mut bot = BotState {
        has_token: false,
        stopped: false,
        no_api: false,
        bot: None,
        pairing_until: None,
        access: Some(access.clone()),
        tape: None,
        station: Some(Box::new(status.clone())),
    };
    let mut station = super::super::StationJobs::default();
    station.observe_bot(bot.clone());
    assert!(
        station
            .groups_sync
            .take_job(
                false,
                target(),
                Some(&access),
                &groups,
                &here,
                super::listing(station.bot.as_ref())
            )
            .is_some()
    );
    station.groups_sync.translation_seen(1);
    bot.station = None;
    station.observe_bot(bot.clone());
    assert_eq!(station.cores_seen, Some(Some(listing)));
    assert!(
        station
            .groups_sync
            .take_job(
                false,
                target(),
                Some(&access),
                &groups,
                &here,
                super::listing(station.bot.as_ref())
            )
            .is_none()
    );
    bot.station = Some(Box::new(status));
    station.observe_bot(bot);
    assert!(
        station
            .groups_sync
            .take_job(
                false,
                target(),
                Some(&access),
                &groups,
                &here,
                super::listing(station.bot.as_ref())
            )
            .is_some()
    );
}

/// Empty, unread, unsupported, or already matching groups must never overwrite station state.
#[test]
fn groups_sync_requires_nonempty_known_supported_different_groups() {
    let (groups, here, listing, mut access) = fixture();
    let mut sync = GroupsSync::default();
    assert!(
        sync.take_job(false, target(), Some(&access), &[], &here, &listing)
            .is_none()
    );
    assert!(
        sync.take_job(false, None, Some(&access), &groups, &here, &listing)
            .is_none()
    );
    assert!(
        sync.take_job(false, target(), None, &groups, &here, &listing)
            .is_none()
    );
    access.groups = None;
    assert!(
        sync.take_job(false, target(), Some(&access), &groups, &here, &listing)
            .is_none()
    );
    access.groups = Some(vec![CoreGroup {
        name: "GROUP_A".into(),
        cores: vec![11],
    }]);
    assert!(
        sync.take_job(false, target(), Some(&access), &groups, &here, &listing)
            .is_none()
    );
    access.groups.as_mut().unwrap()[0].cores = vec![1];
    assert!(
        sync.take_job(false, target(), Some(&access), &groups, &here, &listing)
            .is_some()
    );
}

/// Sorting group names would lose a user's reorder, while sorting members avoids false sends.
#[test]
fn groups_sync_signature_preserves_group_order_only() {
    let a = CoreGroup {
        name: "GROUP_A".into(),
        cores: vec![1, 2],
    };
    let b = CoreGroup {
        name: "GROUP_B".into(),
        cores: vec![3],
    };
    let reversed = CoreGroup {
        cores: vec![2, 1],
        ..a.clone()
    };
    assert!(same_groups(&[a.clone(), b.clone()], &[reversed, b.clone()]));
    assert!(!same_groups(&[a.clone(), b.clone()], &[b, a]));
}
