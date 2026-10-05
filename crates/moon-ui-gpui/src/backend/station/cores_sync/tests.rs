//! Synthetic endpoint fixtures pin station identity independently of terminal numbering.
use super::{Counts, LocalCore, RowState, Upsert, bulk, local_cores, reconcile, station_uid_for};
use moon_core::config::{AppConfig, Secret, ServerConfig};
use moon_core::station_api::ListedCore;

/// Frozen endpoint export lets the production local projection run without a network connection.
fn overridden(text: &str) -> LocalCore {
    let key = "sX85BQAAAAD4HMdln7gLXlN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AArwH8gJLfmM/47yuKci+sFrrtNibJShbRnc1HGycnqLRazhICIMdoPAhGryNcv1KZClUCEhH6mRG/Np81EodJlA=="; // gitleaks:allow
    super::local_core(3, "Fixture", &Secret::new(key), text)
}

/// Key-only matching would duplicate a pushed override, and lose trace/grant station identity.
#[test]
fn effective_hostname_matches_station_identity_without_resolution() {
    let here = overridden("Core.Example.Invalid:5020");
    let station = ListedCore {
        uid: 9,
        name: "Fixture".into(),
        address: Some("core.example.invalid:5020".into()),
        key_fp: Some(here.key_fp.clone()),
        endpoint_override: Some("Core.Example.Invalid:5020".into()),
    };
    let rows = reconcile(
        std::slice::from_ref(&here),
        std::slice::from_ref(&station),
        None,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, RowState::Same);
    assert_eq!(rows[0].station_uid, Some(9));
    assert!(bulk(&rows).is_empty());
    assert_eq!(station_uid_for(3, &[here], &[station]), Some(9));
}

/// Removing the credential fallback would classify an override edit as an add plus a removal.
#[test]
fn override_only_edit_updates_existing_uid_and_clear_is_an_update() {
    for (old, new) in [
        ("", "203.0.113.7:5020"),
        ("203.0.113.7:5020", ""),
        ("203.0.113.7:5020", "core.example.invalid:5020"),
        ("", "host:0"),
    ] {
        let previous = overridden(old);
        let here = overridden(new);
        let remote = ListedCore {
            uid: 9,
            name: here.name.clone(),
            address: previous.address,
            key_fp: Some(here.key_fp.clone()),
            endpoint_override: Some(old.into()),
        };
        let rows = reconcile(&[here], &[remote], None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, RowState::KeyDiffers);
        assert_eq!(
            bulk(&rows),
            [Upsert {
                terminal_uid: 3,
                station_uid: 9,
                add: false
            }]
        );
    }
}

/// Surfacing unsupported override differences on every old-station read would invite endless retries.
#[test]
fn old_station_ignores_override_differences_on_repeated_reads() {
    let here = overridden("core.example.invalid:5020");
    let remote = ListedCore {
        uid: 9,
        name: here.name.clone(),
        address: Some("198.51.100.42:4321".into()),
        key_fp: Some(here.key_fp.clone()),
        endpoint_override: None,
    };
    for _ in 0..3 {
        let rows = reconcile(
            std::slice::from_ref(&here),
            std::slice::from_ref(&remote),
            None,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, RowState::Same);
        assert_eq!(rows[0].address.as_deref(), Some("198.51.100.42:4321"));
        assert!(bulk(&rows).is_empty());
    }
    assert_eq!(station_uid_for(3, &[here], &[remote]), Some(9));
}

/// A fingerprint fallback before address reservation could steal another row's live endpoint match.
#[test]
fn address_matches_win_and_ambiguous_keys_do_not_move_history() {
    let moved = overridden("203.0.113.7:5020");
    let previous = overridden("");
    let old = ListedCore {
        uid: 1,
        name: moved.name.clone(),
        address: previous.address,
        key_fp: Some(moved.key_fp.clone()),
        endpoint_override: Some(String::new()),
    };
    let destination = ListedCore {
        uid: 9,
        address: moved.address.clone(),
        key_fp: Some("another-key".into()),
        ..old.clone()
    };
    let rows = reconcile(
        std::slice::from_ref(&moved),
        &[old.clone(), destination],
        None,
    );
    assert_eq!(rows[0].state, RowState::OnlyOnStation);
    assert_eq!(rows[1].terminal_uid, Some(3));
    assert_eq!(bulk(&rows)[0].station_uid, 9);
    let mut duplicate = moved.clone();
    duplicate.uid = 4;
    let rows = reconcile(&[moved, duplicate], &[old], None);
    assert_eq!(rows[0].state, RowState::OnlyOnStation);
    assert!(bulk(&rows).iter().all(|change| change.add));
}

/// Reusing an unmatched terminal uid would attach a new address to a removed core's reports.
#[test]
fn removed_and_upgraded_station_uids_are_never_allocated_again() {
    let rows = reconcile(&[local(7, 2)], &[remote(3, 1)], Some(7));
    assert_eq!(bulk(&rows)[0].station_uid, 8);
    let rows = reconcile(&[local(7, 2)], &[remote(9, 1)], Some(12));
    assert_eq!(bulk(&rows)[0].station_uid, 13);
    let rows = reconcile(&[local(7, 2), local(8, 3)], &[remote(9, 1)], Some(12));
    assert_eq!(
        bulk(&rows)
            .iter()
            .map(|change| change.station_uid)
            .collect::<Vec<_>>(),
        [13, 14]
    );
}

/// A missing wire field still allocates above both sets, even for an unused terminal uid.
#[test]
fn old_station_fallback_never_allocates_below_either_current_set() {
    let rows = reconcile(&[local(7, 2), local(20, 3)], &[remote(9, 1)], None);
    assert_eq!(
        bulk(&rows)
            .iter()
            .map(|change| change.station_uid)
            .collect::<Vec<_>>(),
        [21, 22]
    );
    let rows = reconcile(&[local(1, 2)], &[], Some(i64::MAX as u64));
    assert!(bulk(&rows).is_empty());
}

/// Build a local comparison fixture without credentials or connections.
fn local(uid: u64, endpoint: u64) -> LocalCore {
    LocalCore {
        endpoint_override: String::new(),
        key_address: Some(format!("198.51.100.{endpoint}:4510")),
        uid,
        name: format!("Core {endpoint}"),
        address: Some(format!("198.51.100.{endpoint}:4510")),
        key_fp: format!("fp-{endpoint}"),
    }
}

/// Build an independently numbered station entry at the synthetic endpoint.
fn remote(uid: u64, endpoint: u64) -> ListedCore {
    ListedCore {
        endpoint_override: None,
        uid,
        name: format!("Core {endpoint}"),
        address: Some(format!("198.51.100.{endpoint}:4510")),
        key_fp: Some(format!("fp-{endpoint}")),
    }
}

/// Bulk replacement would destroy unseen station cores; matching by uid would duplicate cores.
#[test]
fn synchronized_added_and_old_terminals_only_push_local_changes() {
    let station: Vec<_> = (1..=4).map(|n| remote(n * 10, n)).collect();
    let here: Vec<_> = (1..=4).map(|n| local(n, n)).collect();
    let rows = reconcile(&here, &station, None);
    assert!(rows.iter().all(|row| row.state == RowState::Same));
    assert!(bulk(&rows).is_empty());
    assert_eq!(
        Counts::from_rows(&rows),
        Counts {
            on_station: 4,
            here: 4,
            same: 4,
            ..Counts::default()
        }
    );
    let mut added = here.clone();
    added.push(local(5, 5));
    let rows = reconcile(&added, &station, None);
    assert_eq!(
        bulk(&rows),
        [Upsert {
            terminal_uid: 5,
            station_uid: 41,
            add: true
        }]
    );
    assert_eq!(Counts::from_rows(&rows).same, 4);
    let rows = reconcile(&[local(1, 1), local(2, 2), local(5, 5)], &station, None);
    assert_eq!(
        Counts::from_rows(&rows),
        Counts {
            on_station: 4,
            here: 3,
            same: 2,
            only_here: 1,
            only_station: 2,
            pushable: 1,
            differs: 0
        }
    );
    assert_eq!(
        bulk(&rows),
        [Upsert {
            terminal_uid: 5,
            station_uid: 41,
            add: true
        }]
    );
    assert_eq!(
        rows.iter().map(|row| row.station_uid).collect::<Vec<_>>(),
        [Some(10), Some(20), Some(30), Some(40), Some(41)]
    );
}

/// Name edits and new keys must reuse address-matched station uids; key changes win both edits.
#[test]
fn rename_and_key_change_preserve_station_identity_and_previous_name() {
    let mut renamed = local(3, 1);
    renamed.name = "Renamed".into();
    let mut rekeyed = local(9, 2);
    rekeyed.name = "New name and key".into();
    rekeyed.key_fp = "new-fp".into();
    let rows = reconcile(&[renamed, rekeyed], &[remote(7, 2), remote(5, 1)], None);
    assert_eq!(rows[0].state, RowState::NameDiffers);
    assert_eq!(rows[0].name, "Renamed");
    assert_eq!(rows[0].station_name.as_deref(), Some("Core 1"));
    assert_eq!(rows[0].address.as_deref(), Some("198.51.100.1:4510"));
    assert_eq!(rows[1].state, RowState::KeyDiffers);
    assert_eq!(
        bulk(&rows),
        [
            Upsert {
                terminal_uid: 3,
                station_uid: 5,
                add: false
            },
            Upsert {
                terminal_uid: 9,
                station_uid: 7,
                add: false
            }
        ]
    );
}

/// C2: an apparent terminal add at an existing address is an update using the station uid.
#[test]
fn an_address_match_is_never_an_add_even_with_different_uids() {
    let mut core = local(3, 1);
    core.key_fp = "replacement".into();
    let rows = reconcile(&[core], &[remote(9, 1)], None);
    assert_eq!(
        bulk(&rows),
        [Upsert {
            terminal_uid: 3,
            station_uid: 9,
            add: false
        }]
    );
}

/// C3: collisions with a different address allocate above both sets without overwriting any uid.
#[test]
fn colliding_local_uids_allocate_fresh_station_uids() {
    let here = [local(3, 1), local(8, 8), local(5, 5)];
    let rows = reconcile(&here, &[remote(5, 6), remote(3, 2)], None);
    assert_eq!(
        bulk(&rows),
        [
            Upsert {
                terminal_uid: 3,
                station_uid: 9,
                add: true
            },
            Upsert {
                terminal_uid: 5,
                station_uid: 10,
                add: true
            },
            Upsert {
                terminal_uid: 8,
                station_uid: 11,
                add: true
            }
        ]
    );
    assert_eq!(rows[0].state, RowState::OnlyOnStation);
    assert_eq!(rows[1].station_uid, Some(5));
}

/// Equating missing addresses or matching duplicates by input order would mix core histories.
#[test]
fn unknown_addresses_never_match_and_duplicates_match_by_ascending_uid() {
    let mut unknown = remote(1, 1);
    unknown.address = None;
    let mut local_unknown = local(1, 1);
    local_unknown.address = None;
    let rows = reconcile(&[local_unknown], &[unknown], None);
    assert_eq!(
        (rows[0].state, rows[1].state),
        (RowState::OnlyOnStation, RowState::OnlyHere)
    );
    let here = [local(7, 1), local(2, 1), local(8, 1)];
    let rows = reconcile(&here, &[remote(9, 1), remote(3, 1)], None);
    assert_eq!(
        rows.iter()
            .map(|row| (row.terminal_uid, row.station_uid))
            .collect::<Vec<_>>(),
        [(Some(2), Some(3)), (Some(7), Some(9)), (Some(8), Some(10))]
    );
}

/// Sending local uids to the archive retrieves another core's traces when station uids cross.
#[test]
fn trace_mapping_handles_different_and_crossed_uids_and_skips_unmatched_cores() {
    let here = [local(3, 1), local(9, 2), local(7, 7)];
    assert_eq!(station_uid_for(3, &here, &[remote(12, 1)]), Some(12));
    let station = [remote(3, 2), remote(9, 1)];
    assert_eq!(station_uid_for(3, &here, &station), Some(9));
    assert_eq!(station_uid_for(9, &here, &station), Some(3));
    assert_eq!(station_uid_for(7, &here, &station), None);
}

/// Including disabled, synthetic or keyless servers would expose cores the user cannot push.
#[test]
fn local_comparison_only_includes_active_real_credentials() {
    let mut server: ServerConfig = serde_json::from_str("{\"id\":1}").unwrap();
    server.uid = 3;
    server.active = true;
    server.key = Secret::new("synthetic-key-fixture");
    let mut inactive = server.clone();
    inactive.uid = 5;
    inactive.active = false;
    let mut synthetic = server.clone();
    synthetic.uid = 7;
    synthetic.synthetic = true;
    let mut keyless = server.clone();
    keyless.uid = 9;
    keyless.key = Secret::default();
    let cfg = AppConfig::headless(vec![inactive, synthetic, keyless, server]);
    assert_eq!(
        local_cores(&cfg)
            .iter()
            .map(|core| core.uid)
            .collect::<Vec<_>>(),
        [3]
    );
}

/// Guessing before the first status read can request another core's history on a new station.
#[test]
fn unread_trace_listing_never_maps_a_request() {
    let here = [local(3, 1)];
    let listing = [remote(9, 1)];
    assert_eq!(super::trace_uid(3, &here, None), None);
    assert_eq!(super::trace_uid(3, &here, Some(None)), Some(3));
    assert_eq!(super::trace_uid(3, &here, Some(Some(&listing))), Some(9));
}

/// Duplicate addresses on either side must never cross-file another identity's history.
#[test]
fn ambiguous_trace_addresses_are_skipped() {
    assert_eq!(
        station_uid_for(3, &[local(3, 1)], &[remote(9, 1), remote(12, 1)]),
        None
    );
    assert_eq!(
        station_uid_for(3, &[local(3, 1), local(5, 1)], &[remote(9, 1)]),
        None
    );
    assert_eq!(station_uid_for(3, &[local(3, 1)], &[remote(9, 1)]), Some(9));
}

/// An exhausted signed TOML uid range must disable adds without panicking in render.
#[test]
fn allocation_is_bounded_by_toml_integer_range() {
    let top = i64::MAX as u64;
    let rows = reconcile(&[local(top, 1)], &[remote(top, 2)], None);
    assert_eq!(rows[1].station_uid, None);
    assert!(bulk(&rows).is_empty());
    assert_eq!(Counts::from_rows(&rows).pushable, 0);
    let rows = reconcile(&[local(top - 1, 1)], &[remote(top - 1, 2)], None);
    assert_eq!(rows[1].station_uid, Some(top));
    assert_eq!(bulk(&rows).len(), 1);
    assert!(bulk(&reconcile(&[local(u64::MAX, 1)], &[], None)).is_empty());
}

/// A stale add or update must not silently become another operation or destination.
#[test]
fn stale_push_selections_are_refused() {
    let wanted = [Upsert {
        terminal_uid: 3,
        station_uid: 9,
        add: false,
    }];
    let renamed = {
        let mut core = local(3, 1);
        core.name = "Renamed".into();
        core
    };
    assert_eq!(
        super::selected_changes(
            &wanted,
            &reconcile(std::slice::from_ref(&renamed), &[remote(9, 1)], None)
        ),
        Some(wanted.to_vec())
    );
    assert!(
        super::selected_changes(
            &wanted,
            &reconcile(std::slice::from_ref(&renamed), &[remote(12, 1)], None)
        )
        .is_none()
    );
    assert!(
        super::selected_changes(
            &wanted,
            &reconcile(std::slice::from_ref(&renamed), &[], None)
        )
        .is_none()
    );
    let add = [Upsert {
        terminal_uid: 3,
        station_uid: 3,
        add: true,
    }];
    assert!(super::selected_changes(&add, &reconcile(&[renamed], &[remote(9, 1)], None)).is_none());
    assert_eq!(
        super::selected_changes(&wanted, &reconcile(&[local(3, 1)], &[remote(9, 1)], None)),
        Some(vec![])
    );
}

/// Removing a reused uid or a newly local core must require a fresh explicit click.
#[test]
fn stale_removal_selections_are_refused() {
    let listing = [remote(9, 1), remote(12, 2)];
    let names = [listing[0].name.clone()];
    let addresses = [listing[0].address.clone()];
    assert!(super::removal_matches(
        &[9],
        &names,
        &addresses,
        &reconcile(&[], &listing, None)
    ));
    assert!(!super::removal_matches(
        &[9],
        &names,
        &addresses,
        &reconcile(&[local(3, 1)], &listing, None)
    ));
    let changed = [remote(9, 3), remote(12, 2)];
    assert!(!super::removal_matches(
        &[9],
        &names,
        &addresses,
        &reconcile(&[], &changed, None)
    ));
    let mut renamed = listing.clone();
    renamed[0].name = "Renamed".into();
    assert!(!super::removal_matches(
        &[9],
        &names,
        &addresses,
        &reconcile(&[], &renamed, None)
    ));
}

/// Disabling a core only changes push eligibility, never the ability to retrieve its history.
#[test]
fn trace_catalog_keeps_disabled_real_cores() {
    let mut server: ServerConfig = serde_json::from_str("{\"id\":1}").unwrap();
    server.uid = 3;
    server.active = false;
    server.key = Secret::new("fixture");
    let mut synthetic = server.clone();
    synthetic.uid = 5;
    synthetic.synthetic = true;
    let cfg = AppConfig::headless(vec![server, synthetic]);
    assert!(local_cores(&cfg).is_empty());
    assert_eq!(
        super::trace_cores(&cfg)
            .iter()
            .map(|core| core.uid)
            .collect::<Vec<_>>(),
        [3]
    );
}
