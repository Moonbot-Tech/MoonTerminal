//! Crossed UID namespaces reproduce group and viewer identity collisions without live hosts.

use super::UidTranslation;
use crate::backend::station::cores_sync::LocalCore;
use moon_core::config::CoreGroup;
use moon_core::station_api::ListedCore;

/// The same two synthetic endpoints have deliberately colliding numeric identities.
fn crossed() -> (Vec<LocalCore>, Vec<ListedCore>) {
    let here: Vec<_> = [(1, "XALPHA"), (2, "XBETA")]
        .into_iter()
        .map(|(uid, name)| LocalCore {
            uid,
            name: name.into(),
            address: Some(format!("{name}.example.invalid:4510")),
            key_address: Some(format!("{name}.example.invalid:4510")),
            endpoint_override: String::new(),
            key_fp: name.into(),
        })
        .collect();
    let listing = here
        .iter()
        .zip([11, 1])
        .map(|(core, uid)| ListedCore {
            uid,
            name: core.name.clone(),
            address: core.address.clone(),
            key_fp: Some(core.key_fp.clone()),
            endpoint_override: Some(String::new()),
        })
        .collect();
    (here, listing)
}

/// Sending raw UID 1 would group XBETA instead of XALPHA; names and order must survive.
#[test]
fn issue_890_groups_use_station_identities() {
    let (here, listing) = crossed();
    let mut groups = vec![
        CoreGroup {
            name: "UNIT_01".into(),
            cores: vec![1],
        },
        CoreGroup {
            name: "UNIT_02".into(),
            cores: vec![2],
        },
    ];
    let mut translation = UidTranslation::new(&here, &listing);
    translation.groups_to_station(&mut groups);
    assert_eq!(groups[0].cores, [11]);
    assert_eq!(groups[1].cores, [1]);
    assert_eq!(
        groups
            .iter()
            .map(|group| group.name.as_str())
            .collect::<Vec<_>>(),
        ["UNIT_01", "UNIT_02"]
    );
    assert_eq!(translation.unmapped(), 0);
}

/// Numeric fallback for missing, archived or duplicate endpoints would silently group another core.
#[test]
fn issue_890_missing_and_ambiguous_members_are_dropped_and_counted_once() {
    let (mut here, mut listing) = crossed();
    let mut duplicate = listing[0].clone();
    duplicate.uid = 12;
    listing.push(duplicate);
    let mut groups = vec![CoreGroup {
        name: "UNIT_01".into(),
        cores: vec![1, 2, 99, 99],
    }];
    let mut translation = UidTranslation::new(&here, &listing);
    translation.groups_to_station(&mut groups);
    assert_eq!(groups[0].cores, [1]);
    assert_eq!(translation.unmapped(), 2);

    let mut duplicate = here[1].clone();
    duplicate.uid = 3;
    here.push(duplicate);
    let mut translation = UidTranslation::new(&here, &listing);
    assert!(translation.uids(&[2, 3], false).is_empty());
    assert_eq!(translation.unmapped(), 2);

    let mut translation = UidTranslation::new(&here, &[]);
    assert!(translation.uids(&[1, 2], false).is_empty());
    assert_eq!(translation.unmapped(), 2);
}

/// Raw grants would expose XBETA after transfer and lose XALPHA on return; pairing and roles survive.
#[test]
fn issue_891_viewer_grants_preserve_identity_in_both_directions() {
    use moon_core::config::{TelegramConfig, telegram_access::TelegramReportAccess};
    use moon_core::station_api::Access;

    let (here, listing) = crossed();
    let mut terminal = TelegramConfig::default();
    terminal.pair_chat(101);
    terminal.pair_chat(202);
    terminal.chat_profile_mut(202).core_uids = vec![1];
    let mut pairing = Access::of(&terminal);
    let mut translation = UidTranslation::new(&here, &listing);
    translation.access(&mut pairing, false);
    let mut station = TelegramConfig::default();
    pairing.apply_to(&mut station);
    assert_eq!(
        station.report_access(101),
        Some(TelegramReportAccess::Owner)
    );
    assert_eq!(
        station.report_access(202),
        Some(TelegramReportAccess::Viewer(vec![11]))
    );
    assert_eq!(station.authorized_chat_ids, [101, 202]);
    assert_eq!(translation.unmapped(), 0);

    let mut translation = UidTranslation::new(&here, &listing);
    translation.access(&mut pairing, true);
    pairing.apply_to(&mut terminal);
    assert_eq!(
        terminal.report_access(202),
        Some(TelegramReportAccess::Viewer(vec![1]))
    );
    assert_eq!(
        terminal.report_access(101),
        Some(TelegramReportAccess::Owner)
    );
    assert_eq!(translation.unmapped(), 0);
    // Station XBETA UID 1 must map to terminal XBETA UID 2, never XALPHA UID 1.
    assert_eq!(translation.uids(&[1], true), [2]);
}

/// Empty, missing and ambiguous mappings must revoke grants instead of falling back to equal UIDs.
#[test]
fn issue_891_unproven_grants_never_widen_viewer_access() {
    use moon_core::config::{TelegramConfig, telegram_access::TelegramReportAccess};
    use moon_core::station_api::Access;

    let (here, listing) = crossed();
    let mut duplicate_remote = listing.clone();
    let mut duplicate = listing[0].clone();
    duplicate.uid = 12;
    duplicate_remote.push(duplicate);
    let mut duplicate_local = here.clone();
    let mut duplicate = here[0].clone();
    duplicate.uid = 3;
    duplicate_local.push(duplicate);
    for (here, listing) in [
        (&here[..], &[][..]),
        (&here, &duplicate_remote),
        (&duplicate_local, &listing),
    ] {
        for (reverse, uid) in [(false, 1), (true, 11)] {
            let mut config = TelegramConfig::default();
            config.pair_chat(101);
            config.pair_chat(202);
            config.chat_profile_mut(202).core_uids = vec![uid, 99];
            let mut pairing = Access::of(&config);
            let mut translation = UidTranslation::new(here, listing);
            translation.access(&mut pairing, reverse);
            pairing.apply_to(&mut config);
            assert_eq!(
                config.report_access(202),
                Some(TelegramReportAccess::Viewer(Vec::new()))
            );
            assert_eq!(config.report_access(101), Some(TelegramReportAccess::Owner));
            assert_eq!(translation.unmapped(), 2);
        }
    }
}
