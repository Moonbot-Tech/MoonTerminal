//! Validation failures retain kinds until the presentation boundary.

use super::{
    core_keys, first_access, picked_core_key, saved_local_cores, show_journal, show_status,
};
use crate::backend::station::cores_sync;
use moon_core::config::{CoreKeyEntry, Secret};
use moon_core::station_api::ListedCore;
use moon_remote::error::StationError;
use moon_remote::station::bot::BotState;

/// Create saved credentials without reading the vault or contacting a server.
fn install_entry(uid: u64, name: &str, active: bool, key: &str) -> CoreKeyEntry {
    CoreKeyEntry {
        uid,
        name: name.into(),
        active,
        key: Secret::new(key),
        transport: None,
        endpoint_override: String::new(),
    }
}

/// A keyless pick must not block a keyed pick; disabled and unpicked cores must never be sent.
#[test]
fn install_selection_sends_enabled_keys_and_retains_skipped_names() {
    let (keys, skipped) = super::install_core_keys(
        vec![
            install_entry(3, "Ready", true, "synthetic-key"),
            install_entry(5, "Needs key", true, ""),
            install_entry(7, "Disabled", false, "synthetic-disabled-key"),
            install_entry(9, "Not picked", true, "synthetic-unpicked-key"),
        ],
        &[3, 5, 7],
    )
    .unwrap();
    assert_eq!(keys.iter().map(|key| key.uid).collect::<Vec<_>>(), [3]);
    assert_eq!(skipped, ["Needs key"]);
}

/// An all-keyless install must refuse before setup with the missing core's typed name.
#[test]
fn all_keyless_install_refuses_before_remote_work() {
    let error = super::install_core_keys(vec![install_entry(3, "Needs key", true, "")], &[3])
        .err()
        .unwrap();
    assert_eq!(
        error.downcast_ref::<StationError>(),
        Some(&StationError::CoreWithoutKey("Needs key".into()))
    );
}

/// Missing picks must remain a stale-selection error rather than silently installing a subset.
#[test]
fn missing_install_pick_keeps_its_typed_failure() {
    let error = super::install_core_keys(
        vec![install_entry(3, "Ready", true, "synthetic-key")],
        &[3, 5],
    )
    .err()
    .unwrap();
    assert_eq!(
        error.downcast_ref::<StationError>(),
        Some(&StationError::CoreMissing(5))
    );
}

/// Losing the persisted override in picked/upsert projections would make preview and push disagree.
#[test]
fn persisted_override_survives_station_uid_translation() {
    let entries = [CoreKeyEntry {
        uid: 3,
        name: "Fixture".into(),
        key: Secret::new("synthetic-core-key"),
        transport: None,
        active: true,
        endpoint_override: "Core.Example.Invalid:5020".into(),
    }];
    let picks = [cores_sync::Upsert {
        terminal_uid: 3,
        station_uid: 9,
        add: false,
    }];
    let keys = super::upsert_keys(&entries, &picks).unwrap();
    assert_eq!(keys[0].uid, 9);
    assert_eq!(keys[0].endpoint_override, "Core.Example.Invalid:5020");
    assert_eq!(
        saved_local_cores(&entries, &[3])[0].endpoint_override,
        "Core.Example.Invalid:5020"
    );
}

/// Collect the exact lines the production job would send to Settings.
fn lines(show: impl FnOnce(&mut dyn FnMut(moon_remote::progress::Progress))) -> Vec<String> {
    let mut lines = Vec::new();
    show(&mut |event| {
        if let Some(text) = super::super::text::progress(event) {
            lines.push(text);
        }
    });
    lines
}

/// Returning a formatted string here makes an empty-password form show English in all locales.
#[test]
fn missing_password_and_core_selection_are_typed() {
    let failure = first_access("root".into(), None, None).err().unwrap();
    assert_eq!(
        failure.downcast_ref::<StationError>(),
        Some(&StationError::EmptyPassword)
    );
    let failure = core_keys(&[]).err().unwrap();
    assert_eq!(
        failure.downcast_ref::<StationError>(),
        Some(&StationError::NoCorePicked)
    );
}

/// Flattening lookup errors loses the missing uid or core name needed for recovery guidance.
#[test]
fn missing_and_keyless_cores_fail_before_server_work() {
    let entries = [CoreKeyEntry {
        endpoint_override: String::new(),
        uid: 17,
        name: "Fixture core".into(),
        key: Secret::default(),
        transport: None,
        active: true,
    }];
    let missing = picked_core_key(&entries, 19).err().unwrap();
    assert_eq!(
        missing.downcast_ref::<StationError>(),
        Some(&StationError::CoreMissing(19))
    );
    let keyless = picked_core_key(&entries, 17).err().unwrap();
    assert_eq!(
        keyless.downcast_ref::<StationError>(),
        Some(&StationError::CoreWithoutKey("Fixture core".into()))
    );
    assert!(!format!("{keyless:#}").contains("Secret"));
}

/// Suppressing helper tokens must not suppress stopped/no-API Status feedback in Station.
#[test]
fn status_without_api_still_reaches_settings() {
    let _locale = crate::test_locale::force("en");
    let stopped = BotState {
        stopped: true,
        ..BotState::default()
    };
    assert_eq!(
        lines(|say| show_status(&stopped, say)),
        ["The service on the server is stopped."]
    );
    let old = BotState {
        no_api: true,
        ..BotState::default()
    };
    let shown = lines(|say| show_status(&old, say));
    assert!(shown[0].contains("Update the service"));
}

/// Prefixing journal entries with station.detail makes every requested row wrap into noisy prose.
#[test]
fn requested_journal_preserves_raw_lines_without_detail_prefixes() {
    let _locale = crate::test_locale::force("en");
    let shown = lines(|say| show_journal("synthetic journal entry\nsecond entry", say));
    assert_eq!(
        shown,
        [
            "Station journal:",
            "synthetic journal entry",
            "second entry"
        ]
    );
}

/// An excluded synthetic uid must not participate in matching or inflate a fresh uid allocation.
#[test]
fn excluded_persisted_keys_do_not_raise_allocated_uids() {
    let all = [3, 1000].map(|uid| CoreKeyEntry {
        endpoint_override: String::new(),
        uid,
        name: format!("Core {uid}"),
        key: Secret::new("synthetic-fixture"),
        transport: None,
        active: true,
    });
    let here = saved_local_cores(&all, &[3]);
    assert_eq!(here.iter().map(|core| core.uid).collect::<Vec<_>>(), [3]);
    let listing = [ListedCore {
        endpoint_override: None,
        uid: 3,
        name: "Other address".into(),
        address: Some("198.51.100.9:4510".into()),
        key_fp: None,
    }];
    let rows = cores_sync::reconcile(&here, &listing, None);
    assert_eq!(cores_sync::bulk(&rows)[0].station_uid, 4);
    assert!(saved_local_cores(&all, &[]).is_empty());
}

/// Treating reinstall picks as unconditional adds refuses an existing address or overwrites a uid.
#[test]
fn install_reconciles_existing_addresses_and_preserves_station_only_cores() {
    // Frozen TESTKEY V1 export from the endpoint contract fixture.
    let key = "sX85BQAAAAD4HMdln7gLXlN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AArwH8gJLfmM/47yuKci+sFrrtNibJShbRnc1HGycnqLRazhICIMdoPAhGryNcv1KZClUCEhH6mRG/Np81EodJlA=="; // gitleaks:allow
    let keys = [3, 5].map(|uid| moon_remote::station::CoreKey {
        endpoint_override: String::new(),
        uid,
        name: format!("Local {uid}"),
        transport: None,
        key: Secret::new(key),
    });
    let address = moon_core::station_api::core_address(keys[0].key.expose(), "");
    assert!(address.is_some());
    let listing = [
        ListedCore {
            endpoint_override: None,
            uid: 9,
            name: "Old name".into(),
            address,
            key_fp: None,
        },
        ListedCore {
            endpoint_override: None,
            uid: 5,
            name: "Station only".into(),
            address: Some("198.51.100.9:4510".into()),
            key_fp: None,
        },
    ];
    assert_eq!(
        super::install_changes(&keys[..1], Some(&listing), None).unwrap(),
        [cores_sync::Upsert {
            terminal_uid: 3,
            station_uid: 9,
            add: false,
        }]
    );
    assert_eq!(
        super::install_changes(&keys[1..], Some(&listing[1..]), None).unwrap(),
        [cores_sync::Upsert {
            terminal_uid: 5,
            station_uid: 6,
            add: true,
        }]
    );
    assert_eq!(
        super::install_changes(&keys[..1], None, None).unwrap(),
        [cores_sync::Upsert {
            terminal_uid: 3,
            station_uid: 3,
            add: true,
        }]
    );
}
