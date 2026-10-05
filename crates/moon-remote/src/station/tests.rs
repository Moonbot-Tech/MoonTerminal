use super::*;

/// Sending terminal uid 3 directly would reject a valid CLI add to a station whose floor is 12.
#[test]
fn cli_adds_allocate_above_retired_config_reports_and_both_uid_sets() {
    let listing = [moon_core::station_api::ListedCore {
        uid: 9,
        name: "Keep".into(),
        address: None,
        key_fp: None,
    }];
    let cores = [
        synthetic_core(3, "New", None),
        synthetic_core(7, "Other", None),
    ];
    let allocated = allocate_cli_cores(&cores, &listing, 12).unwrap();
    assert_eq!(
        allocated.iter().map(|core| core.uid).collect::<Vec<_>>(),
        [13, 14]
    );
    assert!(
        merge_cores_with_add_flags(
            Some("[[core]]\nuid = 9\nname = 'Keep'\n"),
            &allocated,
            &[true, true],
            None,
            Some(12)
        )
        .is_ok()
    );
    let old = allocate_cli_cores(&[synthetic_core(20, "New", None)], &listing, 0).unwrap();
    assert_eq!(old[0].uid, 21);
    let top = i64::MAX as u64;
    assert_eq!(
        allocate_cli_cores(&cores[..1], &[], top - 1).unwrap()[0].uid,
        top
    );
    assert!(allocate_cli_cores(&cores[..1], &[], top).is_err());
    assert!(allocate_cli_cores(&[synthetic_core(0, "Invalid", None)], &[], 12).is_err());
}

/// Renumbering a listed address as an add would open duplicate connections and split history.
#[test]
fn cli_adds_do_not_duplicate_an_existing_station_address() {
    // Frozen synthetic key from the endpoint wire fixture; no real credentials or connections.
    let key = "sX85BQAAAAD4HMdln7gLXlN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AArwH8gJLfmM/47yuKci+sFrrtNibJShbRnc1HGycnqLRazhICIMdoPAhGryNcv1KZClUCEhH6mRG/Np81EodJlA=="; // gitleaks:allow
    let mut core = synthetic_core(3, "Already listed", None);
    core.key = Secret::new(key);
    let listing = [moon_core::station_api::ListedCore {
        uid: 9,
        name: "Keep".into(),
        address: Some("198.51.100.42:4321".into()),
        key_fp: None,
    }];
    assert!(allocate_cli_cores(&[core], &listing, 12).is_err());
}

/// Dropping the maximum during removal would reuse a retired uid after the service restarts.
#[test]
fn removed_uids_remain_retired_in_persisted_station_config() {
    let original = "[[core]]\nuid = 3\nname = 'Keep'\n[[core]]\nuid = 7\nname = 'Remove'\n";
    let (removed, _) = without_cores(original, &[7]).unwrap();
    assert!(
        merge_cores_with_add_flags(
            Some(&removed),
            &[synthetic_core(7, "New", None)],
            &[true],
            None,
            None
        )
        .is_err()
    );
    let next = merge_cores_with_add_flags(
        Some(&removed),
        &[synthetic_core(8, "New", None)],
        &[true],
        None,
        None,
    )
    .unwrap();
    let persisted: toml::Table = toml::from_str(&next).unwrap();
    assert_eq!(persisted["core_uid_high_water"].as_integer(), Some(8));
    let (removed_again, _) = without_cores(&next, &[8]).unwrap();
    assert!(
        merge_cores_with_add_flags(
            Some(&removed_again),
            &[synthetic_core(8, "Another", None)],
            &[true],
            None,
            None
        )
        .is_err()
    );
}

/// A report-only retired uid from an upgraded station must be checked before credentials change.
#[test]
fn upgrade_report_floor_is_enforced_and_persisted_by_pushes() {
    let config = "[[core]]\nuid = 9\nname = 'Keep'\n";
    assert!(
        merge_cores_with_add_flags(
            Some(config),
            &[synthetic_core(12, "New", None)],
            &[true],
            None,
            Some(12)
        )
        .is_err()
    );
    let next = merge_cores_with_add_flags(
        Some(config),
        &[synthetic_core(13, "New", None)],
        &[true],
        None,
        Some(12),
    )
    .unwrap();
    let (removed, _) = without_cores(&next, &[13]).unwrap();
    assert!(
        merge_cores_with_add_flags(
            Some(&removed),
            &[synthetic_core(12, "Other", None)],
            &[true],
            None,
            None
        )
        .is_err()
    );
    assert!(
        merge_cores_with_add_flags(
            Some(config),
            &[synthetic_core(9, "Rename", None)],
            &[false],
            None,
            Some(12)
        )
        .is_ok()
    );
}

/// C3: validating after put-cred overwrites an unrelated station core before refusing the add.
#[test]
fn an_add_collision_is_refused_before_any_credential_write() {
    let config = "[[core]]\nuid = 3\nname = 'Existing'\n";
    let cores = [
        synthetic_core(9, "New", None),
        synthetic_core(3, "Collision", None),
    ];
    assert_eq!(
        merge_cores_with_add_flags(Some(config), &cores, &[true, true], None, None)
            .unwrap_err()
            .to_string(),
        "station changed, refresh"
    );
    merge_cores_with_add_flags(Some(config), &cores, &[true, false], None, None).unwrap();
    merge_cores_with_add_flags(None, &cores, &[true, true], None, None).unwrap();
    let source = include_str!("../station.rs");
    let cli_push = source
        .split("pub fn push_cores(")
        .nth(1)
        .unwrap()
        .split("pub fn push_cores_with_add_flags(")
        .next()
        .unwrap();
    assert!(cli_push.contains("&vec![true; cores.len()]"));
    let push = source
        .split("pub fn push_cores_with_add_flags(")
        .nth(1)
        .unwrap()
        .split("/// Reject stale")
        .next()
        .unwrap();
    assert!(
        push.find("merge_cores_with_add_flags(current.as_deref()")
            .unwrap()
            < push.find("put-cred").unwrap()
    );
}

/// A made-up core credential for config-only tests; never connects.
fn synthetic_core(uid: u64, name: &str, transport: Option<TransportVersion>) -> CoreKey {
    CoreKey {
        uid,
        name: name.into(),
        transport,
        key: Secret::new("synthetic-core-key"),
    }
}

/// Whole-set replacement loses station-only cores; replacing a matching table loses unknown fields.
#[test]
fn a_cores_merge_updates_only_named_uids_and_preserves_sections() {
    let current = "[[core]]\nuid = 3\nname = 'Three'\n[[core]]\nuid = 5\nname = 'Five'\ntransport = 'v1'\nactive = false\nfuture = 'kept'\n[[core]]\nuid = 7\nname = 'Seven'\n[tape]\nmargin_s = 300\n[telegram]\nmini_app = true\n[update]\nauto = false\n";
    let before: toml::Value = toml::from_str(current).unwrap();
    let upsert = [
        synthetic_core(5, "Renamed", None),
        synthetic_core(9, "New", Some(TransportVersion::V1)),
    ];
    let text = merge_cores(Some(current), &upsert, None).unwrap();
    let after: toml::Value = toml::from_str(&text).unwrap();
    let cores = after["core"].as_array().unwrap();
    assert_eq!(
        cores
            .iter()
            .map(|core| core["uid"].as_integer().unwrap())
            .collect::<Vec<_>>(),
        [3, 5, 7, 9]
    );
    assert_eq!(cores[0], before["core"][0]);
    assert_eq!(cores[2], before["core"][2]);
    assert_eq!(cores[1]["name"].as_str(), Some("Renamed"));
    assert_eq!(cores[1]["active"].as_bool(), Some(true));
    assert_eq!(cores[1]["future"].as_str(), Some("kept"));
    assert!(cores[1].get("transport").is_none());
    assert_eq!(cores[3]["transport"].as_str(), Some("v1"));
    for section in ["telegram", "tape", "update"] {
        assert_eq!(after[section], before[section]);
    }
    assert!(!text.contains("synthetic-core-key"));
}

/// Regression tripwire: restoring stale credential cleanup would delete station-only cores during an additive push.
#[test]
fn a_cores_push_never_drops_credentials() {
    let source = include_str!("../station.rs");
    let push = source
        .split("pub fn push_cores(")
        .nth(1)
        .unwrap()
        .split("pub fn remove_cores(")
        .next()
        .unwrap();
    assert!(!push.contains("drop-cred"));
    assert!(!push.contains("commit_cores_config("));
    let removal = source
        .split("pub fn remove_cores(")
        .nth(1)
        .unwrap()
        .split("fn commit_cores_config(")
        .next()
        .unwrap();
    assert!(removal.find("commit_cores_config(").unwrap() < removal.find("drop-cred").unwrap());
}

/// Removing unnamed uids or sections destroys another core's config and station preferences.
#[test]
fn without_cores_drops_only_named_uids() {
    let current = "[[core]]\nuid = 3\nname = 'Three'\n[[core]]\nuid = 5\nname = 'Five'\n[[core]]\nuid = 7\nname = 'Seven'\n[telegram]\nmini_app = true\n[tape]\nmargin_s = 300\n[update]\nauto = false\n";
    let before: toml::Value = toml::from_str(current).unwrap();
    let (changed, removed) = without_cores(current, &[5]).unwrap();
    assert_eq!(removed, vec![5]);
    let after: toml::Value = toml::from_str(&changed).unwrap();
    assert_eq!(
        after["core"].as_array().unwrap(),
        &vec![before["core"][0].clone(), before["core"][2].clone()]
    );
    for section in ["telegram", "tape", "update"] {
        assert_eq!(after[section], before[section]);
    }
}

/// Letting the last core be removed makes the station fail its startup empty-core guard.
#[test]
fn without_cores_refuses_removing_every_core() {
    assert!(without_cores("[[core]]\nuid = 3\nname = 'Three'\n", &[3]).is_err());
}

/// Retrying an absent uid must clean a leftover credential without removing another entry.
#[test]
fn absent_removal_uids_are_idempotent_cleanup_retries() {
    let current = "[[core]]\nuid = 3\nname = 'Keep'\n[[core]]\nuid = 5\nname = 'Remove'\n";
    let (changed, removed) = without_cores(current, &[5, 99]).unwrap();
    assert_eq!(removed, [5]);
    let (retried, removed) = without_cores(&changed, &[5, 99]).unwrap();
    assert!(removed.is_empty());
    assert_eq!(changed, retried);
    let cleaned = std::cell::Cell::new(false);
    commit_cores_config(
        || Ok(false),
        || {
            cleaned.set(true);
            Ok(())
        },
    )
    .unwrap();
    assert!(cleaned.get());
}

/// A drop error must still restart so the service stops using the committed removed entry.
#[test]
fn failed_cleanup_still_restarts_and_can_be_retried() {
    let config = std::cell::RefCell::new(
        "[[core]]\nuid = 3\nname = 'Keep'\n[[core]]\nuid = 9\nname = 'Remove'\n".to_owned(),
    );
    let credential_left = std::cell::Cell::new(true);
    let restarts = std::cell::Cell::new(0);
    let cleanup = commit_cores_config(
        || {
            let (new, removed) = without_cores(&config.borrow(), &[9])?;
            assert_eq!(removed, [9]);
            *config.borrow_mut() = new;
            Ok(true)
        },
        || anyhow::bail!("drop failed"),
    );
    let result = finish_cores_cleanup(cleanup, || {
        restarts.set(restarts.get() + 1);
        Ok(())
    });
    assert_eq!(result.unwrap_err().to_string(), "drop failed");
    assert_eq!(restarts.get(), 1);
    assert!(credential_left.get());
    let committed = config.borrow().clone();
    let cleanup = commit_cores_config(
        || {
            let (new, removed) = without_cores(&config.borrow(), &[9])?;
            assert!(removed.is_empty());
            assert_eq!(new, committed);
            Ok(false)
        },
        || {
            credential_left.set(false);
            Ok(())
        },
    );
    finish_cores_cleanup(cleanup, || {
        restarts.set(restarts.get() + 1);
        Ok(())
    })
    .unwrap();
    assert!(!credential_left.get());
    assert_eq!(restarts.get(), 2);
}

/// A stale update must not restore an entry another terminal removed.
#[test]
fn updates_require_an_existing_destination() {
    let core = [synthetic_core(9, "Removed", None)];
    assert!(
        merge_cores_with_add_flags(
            Some("[[core]]\nuid = 3\nname = 'Keep'\n"),
            &core,
            &[false],
            None,
            None
        )
        .is_err()
    );
    assert!(merge_cores_with_add_flags(None, &core, &[false], None, None).is_err());
}

/// Dropping credentials before a failed removal commit strands a still-referenced core on restart.
#[test]
fn a_failed_config_write_never_drops_credentials() {
    let dropped = std::cell::Cell::new(false);
    let current = "[[core]]\nuid = 3\nname = 'Keep'\n[[core]]\nuid = 9\nname = 'Remove'\n";
    let result = commit_cores_config(
        || {
            let _new = without_cores(current, &[9])?;
            anyhow::bail!("synthetic CAS or SSH failure")
        },
        || {
            dropped.set(true);
            Ok(())
        },
    );
    assert!(result.is_err());
    assert!(
        !dropped.get(),
        "all previously referenced keys must survive"
    );
}

/// Reversing removal callbacks lets cleanup observe the old config; cleanup errors must leave
/// the removed references committed and the remaining unused credential harmless.
#[test]
fn credential_cleanup_failure_keeps_the_new_config() {
    let current = "[[core]]\nuid = 3\nname = 'Keep'\n[[core]]\nuid = 9\nname = 'Remove'\n";
    let config = std::cell::RefCell::new(current.to_owned());
    let result = commit_cores_config(
        || {
            *config.borrow_mut() = without_cores(current, &[9])?.0;
            Ok(true)
        },
        || {
            let file: toml::Value = toml::from_str(&config.borrow()).unwrap();
            assert_eq!(file["core"].as_array().unwrap().len(), 1);
            assert_eq!(file["core"][0]["uid"].as_integer(), Some(3));
            anyhow::bail!("synthetic credential cleanup failure")
        },
    );
    assert!(result.is_err());
    let file: toml::Value = toml::from_str(&config.borrow()).unwrap();
    assert_eq!(file["core"].as_array().unwrap().len(), 1);
    assert_eq!(file["core"][0]["uid"].as_integer(), Some(3));
}

/// The file that goes to the server names each core and never carries its key.
#[test]
fn the_station_file_carries_no_key() {
    let cores = [
        CoreKey {
            uid: 3,
            name: "BinF \"1\"".to_owned(),
            transport: Some(TransportVersion::V1),
            key: Secret::new("SECRET-KEY-TEXT"),
        },
        CoreKey {
            uid: 9,
            name: "HL".to_owned(),
            transport: None,
            key: Secret::new("OTHER-KEY"),
        },
    ];
    let text = merge_cores(
        None,
        &cores,
        Some(TapeWindow {
            margin_s: 180,
            long_position_min: 10,
        }),
    )
    .unwrap();
    assert!(!text.contains("SECRET-KEY-TEXT") && !text.contains("OTHER-KEY"));
    assert!(!text.lines().any(|l| l.trim_start().starts_with("key")));

    let back: toml::Value = toml::from_str(&text).unwrap();
    let list = back["core"].as_array().unwrap();
    assert_eq!(list[0]["uid"].as_integer(), Some(3));
    assert_eq!(list[0]["name"].as_str(), Some("BinF \"1\""));
    assert_eq!(list[0]["transport"].as_str(), Some("v1"));
    assert!(list[1].get("transport").is_none());
    assert_eq!(back["tape"]["margin_s"].as_integer(), Some(180));
    assert_eq!(back["tape"]["long_position_min"].as_integer(), Some(10));
    assert!(
        toml::from_str::<toml::Value>(&merge_cores(None, &cores, None).unwrap())
            .unwrap()
            .get("tape")
            .is_none(),
        "no window from the terminal leaves the station its own"
    );
}

/// Pushing the cores again must not switch the bot off: `[telegram]` rides over from the server's
/// file, and a server without one gets none.
#[test]
fn a_cores_push_keeps_the_bot_section() {
    let upsert = [synthetic_core(10, "A", None)];
    let current = "[[core]]\nuid = 9\nname = 'Old'\n[telegram]\nmini_app = true\nzone = 'UTC'\n";
    let merged: toml::Value =
        toml::from_str(&merge_cores(Some(current), &upsert, None).unwrap()).unwrap();
    assert_eq!(merged["core"][0]["uid"].as_integer(), Some(9));
    assert_eq!(merged["core"][1]["uid"].as_integer(), Some(10));
    assert_eq!(merged["telegram"]["mini_app"].as_bool(), Some(true));
    let fresh: toml::Value = toml::from_str(&merge_cores(None, &upsert, None).unwrap()).unwrap();
    assert_eq!(fresh["core"][0]["uid"].as_integer(), Some(10));
    assert!(fresh.get("telegram").is_none());
}

/// Replacing a station's tape during a cores push changes its recording window without consent.
#[test]
fn a_cores_push_never_replaces_the_stations_window() {
    let current =
        "[[core]]\nuid = 9\nname = 'Old'\n[tape]\nmargin_s = 300\nlong_position_min = 20\n";
    let upsert = [synthetic_core(10, "A", None)];
    let tape = Some(TapeWindow {
        margin_s: 180,
        long_position_min: 10,
    });
    let merged: toml::Value =
        toml::from_str(&merge_cores(Some(current), &upsert, tape).unwrap()).unwrap();
    assert_eq!(merged["tape"]["margin_s"].as_integer(), Some(300));
    assert_eq!(merged["tape"]["long_position_min"].as_integer(), Some(20));
    assert_eq!(merged["core"][0]["uid"].as_integer(), Some(9));
    let bare = "[[core]]\nuid = 9\nname = 'Old'\n";
    let merged: toml::Value =
        toml::from_str(&merge_cores(Some(bare), &upsert, tape).unwrap()).unwrap();
    assert_eq!(merged["tape"]["margin_s"].as_integer(), Some(180));
}

/// `telegram` changes only the fields it is given, and `--off` removes the section whole.
#[test]
fn a_bot_change_touches_only_its_fields() {
    let current = "[[core]]\nuid = 3\nname = \"A\"\n\n[telegram]\nzone = \"Europe/Moscow\"\n";
    let change = BotChange {
        mini_app: Some(true),
        ..BotChange::default()
    };
    let changed: toml::Value =
        toml::from_str(&with_telegram(current, &change, false).unwrap()).unwrap();
    assert_eq!(changed["telegram"]["mini_app"].as_bool(), Some(true));
    assert_eq!(changed["telegram"]["zone"].as_str(), Some("Europe/Moscow"));
    assert_eq!(changed["core"][0]["uid"].as_integer(), Some(3));

    let off: toml::Value =
        toml::from_str(&with_telegram(current, &BotChange::default(), true).unwrap()).unwrap();
    assert!(off.get("telegram").is_none());
    assert_eq!(off["core"][0]["uid"].as_integer(), Some(3));
}

/// A helper older than the compare-and-swap `put-config` prints no `config_cas=`: it is replaced
/// before any write — it would overwrite what another terminal wrote.
#[test]
fn an_old_helper_is_told_by_its_status() {
    // The helper of 2026-09-30 morning had `put-valuation` but no `ctl`.
    assert!(!helper_is_current(
        "active=active
config=yes
creds=core-3
token=no
pairing=no
valuation=no
"
    ));
    // The helper of PR #799/#800: `ctl`, but `put-config` without a base.
    assert!(!helper_is_current(
        "active=active
config=yes
creds=core-3
token=no
pairing=no
valuation=no
api=no
"
    ));
    // The helper of PR #803/#804: `put-config` with a base, but no `update-from-release`.
    assert!(!helper_is_current(
        "active=active
config=yes
creds=core-3
token=no
pairing=no
valuation=no
api=no
config_cas=yes
"
    ));
    // Release updates alone also predate credential export and guarded removal.
    assert!(!helper_is_current(
        "release_update=yes
bot_return=no
"
    ));
    assert!(!helper_is_current(
        "release_update=yes
"
    ));
    assert!(!helper_is_current(
        "active=active
config=yes
creds=core-3
token=no
pairing=no
valuation=no
api=no
config_cas=yes
update_path=active
release_update=yes
bot_return=yes
"
    ));
}

/// The helper's status really prints the marker the terminal looks for.
#[test]
fn the_helper_prints_its_marker() {
    assert!(crate::script::HELPER.contains("echo \"bot_return=yes\""));
    assert!(crate::script::HELPER.contains("echo \"remove_station=yes\""));
    assert!(crate::script::HELPER.contains("echo \"removal_guard=yes\""));
}

/// A changed window rewrites only `[tape]` — the cores and the bot stay as the server has them —
/// and the window the station already has changes nothing, so no reload is sent.
#[test]
fn a_tape_push_changes_only_the_tape_section() {
    let current = "[[core]]\nuid = 9\nname = \"Old\"\n\n[tape]\nmargin_s = 180\nlong_position_min = 10\n\n[telegram]\nmini_app = true\n";
    let same = TapeWindow {
        margin_s: 180,
        long_position_min: 10,
    };
    assert!(with_tape(current, same).unwrap().is_none());

    let moved = TapeWindow {
        margin_s: 300,
        long_position_min: 10,
    };
    let text = with_tape(current, moved)
        .unwrap()
        .expect("a changed window");
    let back: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(back["tape"]["margin_s"].as_integer(), Some(300));
    assert_eq!(back["tape"]["long_position_min"].as_integer(), Some(10));
    assert_eq!(back["core"][0]["uid"].as_integer(), Some(9));
    assert_eq!(back["telegram"]["mini_app"].as_bool(), Some(true));

    let bare = "[[core]]\nuid = 9\nname = \"Old\"\n";
    let text = with_tape(bare, same)
        .unwrap()
        .expect("a file without [tape] gets one");
    let back: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(back["tape"]["margin_s"].as_integer(), Some(180));
}

/// Dropping the removal marker would keep an old helper after Update the service, preventing
/// the user from safely removing their station. Unknown capability values must also refresh.
#[test]
fn helper_refresh_requires_the_removal_capability() {
    assert!(!helper_is_current(
        "remove_station=yes\nremoval_guard=yes\n"
    ));
    assert!(helper_is_current(
        "release_update=yes
bot_return=yes
remove_station=yes
removal_guard=yes
bot_settings_merge=yes
"
    ));
    assert!(!helper_is_current(
        "release_update=yes
remove_station=no
"
    ));
    assert!(!helper_is_current("remove_station=yes\n"));
}

/// The switch is written only when it changes, and only `[update]` is touched; a cores push keeps
/// the server's switch — otherwise sending the cores again would silently switch updates back on.
#[test]
fn the_auto_update_switch_is_written_once_and_kept_by_a_cores_push() {
    let bare =
        "[[core]]\nuid = 3\nname = \"A\"\n\n[tape]\nmargin_s = 180\nlong_position_min = 10\n";
    assert_eq!(
        with_auto_update(bare, true).unwrap(),
        None,
        "no switch already means on"
    );
    let off = with_auto_update(bare, false)
        .unwrap()
        .expect("switched off");
    let parsed: toml::Value = toml::from_str(&off).unwrap();
    assert_eq!(parsed["update"]["auto"].as_bool(), Some(false));
    assert_eq!(parsed["tape"]["margin_s"].as_integer(), Some(180));
    assert_eq!(parsed["core"][0]["uid"].as_integer(), Some(3));
    assert_eq!(with_auto_update(&off, false).unwrap(), None);
    let on: toml::Value =
        toml::from_str(&with_auto_update(&off, true).unwrap().expect("switched on")).unwrap();
    assert_eq!(on["update"]["auto"].as_bool(), Some(true));

    let merged: toml::Value =
        toml::from_str(&merge_cores(Some(&off), &[synthetic_core(4, "B", None)], None).unwrap())
            .unwrap();
    assert_eq!(merged["update"]["auto"].as_bool(), Some(false));
}
