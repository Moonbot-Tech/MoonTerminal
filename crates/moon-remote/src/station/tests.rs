use super::*;

/// Dropping an override during a uid-preserving upsert would keep the station dialing the old key.
#[test]
fn core_override_push_round_trips_updates_and_explicit_clear() {
    let current = "[[core]]\nuid = 3\nname = 'Old'\n[[core]]\nuid = 9\nname = 'Station only'\nendpoint_override = '203.0.113.9:5020'\n";
    let mut core = synthetic_core(3, "Renamed", None);
    core.endpoint_override = "Core.Example.Invalid:5020".into();
    let pushed = peer_cores(&[core], "core_endpoint_override=yes\n");
    let merged = merge_cores_with_add_flags(Some(current), &pushed, &[false], None, None).unwrap();
    let file: toml::Value = toml::from_str(&merged).unwrap();
    let cores = file["core"].as_array().unwrap();
    assert_eq!(cores.len(), 2);
    assert_eq!(cores[0]["uid"].as_integer(), Some(3));
    assert_eq!(
        cores[0]["endpoint_override"].as_str(),
        Some("Core.Example.Invalid:5020")
    );
    assert_eq!(
        cores[1]["endpoint_override"].as_str(),
        Some("203.0.113.9:5020")
    );
    assert!(!merged.contains("synthetic-core-key"));
    let mut cleared = pushed;
    cleared[0].endpoint_override.clear();
    let merged = merge_cores_with_add_flags(Some(&merged), &cleared, &[false], None, None).unwrap();
    let file: toml::Value = toml::from_str(&merged).unwrap();
    assert!(file["core"][0].get("endpoint_override").is_none());
    assert_eq!(
        file["core"][1]["endpoint_override"].as_str(),
        Some("203.0.113.9:5020")
    );
}

/// A helper refresh must not send a field that an older installed binary refuses at startup.
#[test]
fn old_station_pushes_omit_overrides_and_keep_credentials() {
    let mut core = synthetic_core(3, "Fixture", None);
    core.endpoint_override = "core.example.invalid:5020".into();
    for status in [
        "",
        "core_endpoint_override=no\n",
        "core_endpoint_override=unknown\n",
    ] {
        let pushed = peer_cores(std::slice::from_ref(&core), status);
        let config = merge_cores(None, &pushed, None).unwrap();
        assert!(!config.contains("endpoint_override"));
        assert_eq!(pushed[0].key.expose(), "synthetic-core-key");
        assert_eq!(pushed[0].uid, 3);
    }
}

/// A static helper marker would enable an override against an older binary that rejects the field.
#[test]
fn helper_reads_override_capability_from_the_installed_binary() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let helper = crate::script::HELPER
        .replace("/opt/moon-station", "${fixture}/opt")
        .replace("/etc/moon-station", "${fixture}/etc")
        .replace("/var/lib/moon-station", "${fixture}/data")
        .replace("/run/moon-station/api.sock", "${fixture}/api.sock");
    let helper = helper
        .lines()
        .map(|line| {
            if let Some((name, value)) = line
                .split_once('=')
                .filter(|(_, value)| value.starts_with("${fixture}/"))
            {
                return format!("{name}=\"{value}\"");
            }
            line.to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n");
    for (binary, expected) in [
        (None, "no"),
        (Some("exit 1"), "no"),
        (Some("printf 'core_endpoint_override=unknown\\n'"), "no"),
        (Some("printf 'core_endpoint_override=yes\\n'; exit 1"), "no"),
        (Some("printf 'core_endpoint_override=yes\\n'"), "yes"),
    ] {
        let before = r#"
set -eu
fixture=$(mktemp -d)
test -d "$fixture"
trap 'rm -r -- "$fixture"' EXIT
mkdir -p "$fixture/opt/bin" "$fixture/etc/creds" "$fixture/data"
id() { printf '0\n'; }
systemctl() { printf 'inactive\n'; }
"#;
        let binary = binary.map(|code| format!("printf '%s\\n' '#!/bin/sh' \"{code}\" >\"$fixture/opt/bin/moon-station\"\nchmod +x \"$fixture/opt/bin/moon-station\"\n")).unwrap_or_default();
        let code = format!("{before}\n{binary}\nset -- status\n{helper}");
        let mut child = Command::new("sh")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("POSIX sh is required for the synthetic helper fixture");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(code.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            crate::script::value(
                &String::from_utf8(output.stdout).unwrap(),
                "core_endpoint_override"
            ),
            Some(expected)
        );
    }
}

/// Sending terminal uid 3 directly would reject a valid CLI add to a station whose floor is 12.
#[test]
fn cli_adds_allocate_above_retired_config_reports_and_both_uid_sets() {
    let listing = [moon_core::station_api::ListedCore {
        endpoint_override: None,
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
        endpoint_override: None,
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
}

/// Synthetic helper storage: commands enforce only the remote helper's config CAS protocol.
#[derive(Default)]
struct CorePushFixture {
    config: Option<String>,
    credentials: std::collections::HashMap<u64, Vec<u8>>,
}

impl CorePushFixture {
    /// Execute the production commands without SSH, encryption or a running station.
    fn run(&mut self, command: &str, stdin: &[u8]) -> anyhow::Result<crate::ssh::Output> {
        let mut out = crate::ssh::Output {
            status: Some(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        if command == script::helper("get-config", &[]) {
            out.stdout = self.config.as_ref().unwrap().as_bytes().to_vec();
        } else if command.starts_with(&script::helper("put-config", &[])) {
            let base = self.config.as_ref().map_or_else(
                || "none".to_owned(),
                |text| format!("{:x}", Sha256::digest(text.as_bytes())),
            );
            if command == script::helper("put-config", &[&base]) {
                self.config = Some(String::from_utf8(stdin.to_vec()).unwrap());
            } else {
                out.status = Some(3);
                out.stdout = b"config=changed\n".to_vec();
            }
        } else if command == script::helper("put-cred", &["81"]) {
            self.credentials.insert(81, stdin.to_vec());
        } else {
            panic!("unexpected fixture command: {command}");
        }
        Ok(out)
    }
}

/// Moving credential publication before the guarded edit corrupts CORE_A when CORE_B loses a race.
#[test]
fn concurrent_add_refusal_preserves_the_accepted_credential() {
    for exists in [false, true] {
        for race_at_cas in [false, true] {
            let mut fixture = CorePushFixture {
                config: exists.then(|| {
                    "core_uid_high_water = 80\n[[core]]\nuid = 80\nname = 'UNIT_01'\n".into()
                }),
                ..Default::default()
            };
            let mut a = synthetic_core(81, "CORE_A", None);
            a.key = Secret::new("synthetic-key-A");
            let mut b = synthetic_core(81, "CORE_B", None);
            b.key = Secret::new("synthetic-key-B");
            // Both terminals pass preflight against the same snapshot, before A commits.
            for core in [&a, &b] {
                merge_cores_with_add_flags(
                    fixture.config.as_deref(),
                    std::slice::from_ref(core),
                    &[true],
                    None,
                    Some(80),
                )
                .unwrap();
            }
            let accept_a = |fixture: &mut CorePushFixture| {
                write_cores(
                    &mut |command, stdin| fixture.run(command, stdin),
                    exists,
                    std::slice::from_ref(&a),
                    &[true],
                    None,
                    Some(80),
                    &mut |_| {},
                )
                .unwrap();
                assert_eq!(fixture.credentials[&81], b"synthetic-key-A");
            };
            if !race_at_cas {
                accept_a(&mut fixture);
            }
            let mut injected = false;
            let refused = write_cores(
                &mut |command, stdin| {
                    if race_at_cas
                        && !injected
                        && command.starts_with(&script::helper("put-config", &[]))
                    {
                        accept_a(&mut fixture);
                        injected = true;
                    }
                    fixture.run(command, stdin)
                },
                exists,
                &[b],
                &[true],
                None,
                Some(80),
                &mut |_| {},
            )
            .unwrap_err();
            assert_eq!(refused.to_string(), "station changed, refresh");
            assert_eq!(fixture.credentials[&81], b"synthetic-key-A");
            let config: toml::Value = toml::from_str(fixture.config.as_ref().unwrap()).unwrap();
            let accepted = config["core"]
                .as_array()
                .unwrap()
                .iter()
                .find(|core| core["uid"].as_integer() == Some(81))
                .unwrap();
            assert_eq!(accepted["name"].as_str(), Some("CORE_A"));
            assert_eq!(injected, race_at_cas);
        }
    }
}

/// A made-up core credential for config-only tests; never connects.
fn synthetic_core(uid: u64, name: &str, transport: Option<TransportVersion>) -> CoreKey {
    CoreKey {
        endpoint_override: String::new(),
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

/// An older station binary loses the ticked addresses (`peer_cores`), so each such core is named
/// to the user; a supporting binary, or a core without one, says nothing (#616).
#[test]
fn only_dropped_addresses_are_announced() {
    let cores = [
        CoreKey {
            uid: 3,
            name: "Ticked".to_owned(),
            transport: None,
            key: Secret::new("K1"),
            endpoint_override: "core.example.net:5017".to_owned(),
        },
        CoreKey {
            uid: 4,
            name: "Plain".to_owned(),
            transport: None,
            key: Secret::new("K2"),
            endpoint_override: String::new(),
        },
    ];
    assert_eq!(
        super::endpoints_not_sent(&cores, "core_endpoint_override=no\n"),
        ["Ticked"]
    );
    assert!(super::endpoints_not_sent(&cores, "core_endpoint_override=yes\n").is_empty());
}

/// The file that goes to the server names each core and never carries its key.
#[test]
fn the_station_file_carries_no_key() {
    let cores = [
        CoreKey {
            endpoint_override: String::new(),
            uid: 3,
            name: "BinF \"1\"".to_owned(),
            transport: Some(TransportVersion::V1),
            key: Secret::new("SECRET-KEY-TEXT"),
        },
        CoreKey {
            endpoint_override: String::new(),
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
    assert!(with_tape(current, same, None, None).unwrap().is_none());

    let moved = TapeWindow {
        margin_s: 300,
        long_position_min: 10,
    };
    let text = with_tape(current, moved, None, None)
        .unwrap()
        .expect("a changed window");
    let back: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(back["tape"]["margin_s"].as_integer(), Some(300));
    assert_eq!(back["tape"]["long_position_min"].as_integer(), Some(10));
    assert_eq!(back["core"][0]["uid"].as_integer(), Some(9));
    assert_eq!(back["telegram"]["mini_app"].as_bool(), Some(true));

    let bare = "[[core]]\nuid = 9\nname = \"Old\"\n";
    let text = with_tape(bare, same, None, None)
        .unwrap()
        .expect("a file without [tape] gets one");
    let back: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(back["tape"]["margin_s"].as_integer(), Some(180));
}

/// Replacing the whole tape table would reset stored switches or erase future station fields.
#[test]
fn tape_merge_preserves_foreign_fields_and_only_writes_supported_switches() {
    let current = "[tape]\nmargin_s = 180\nlong_position_min = 10\nauto_catchup = false\narchive_catchup = true\nfuture = 'kept'\n";
    let tape = TapeWindow {
        margin_s: 180,
        long_position_min: 10,
    };
    assert!(with_tape(current, tape, None, None).unwrap().is_none());
    let moved = TapeWindow {
        margin_s: 300,
        ..tape
    };
    let text = with_tape(current, moved, None, None).unwrap().unwrap();
    let back: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(back["tape"]["future"].as_str(), Some("kept"));
    assert_eq!(back["tape"]["auto_catchup"].as_bool(), Some(false));
    assert_eq!(back["tape"]["archive_catchup"].as_bool(), Some(true));
    let choices = CatchupSwitches {
        auto: true,
        archives: false,
    };
    let text = with_tape(current, tape, Some(choices), None)
        .unwrap()
        .unwrap();
    let back: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(back["tape"]["future"].as_str(), Some("kept"));
    assert_eq!(back["tape"]["auto_catchup"].as_bool(), Some(true));
    assert_eq!(back["tape"]["archive_catchup"].as_bool(), Some(false));
    assert_eq!(back["tape"]["margin_s"].as_integer(), Some(180));
    assert!(
        with_tape(&text, tape, Some(choices), None)
            .unwrap()
            .is_none()
    );
    let legacy = with_tape("", tape, None, None).unwrap().unwrap();
    let back: toml::Value = toml::from_str(&legacy).unwrap();
    assert!(back["tape"].get("auto_catchup").is_none());
    assert!(back["tape"].get("archive_catchup").is_none());
    assert!(with_tape("tape = 1", tape, None, None).is_err());
}

/// Unsupported pushes must omit retention keys; supported merges must preserve foreign fields.
#[test]
fn with_tape_writes_keep_only_when_supported() {
    let tape = TapeWindow {
        margin_s: 180,
        long_position_min: 10,
    };
    let keep = TapeKeep {
        max_age_days: 90,
        record_emulator: false,
        tuner_only: None,
        min_tail_s: None,
    };
    let legacy = with_tape("", tape, None, None).unwrap().unwrap();
    let parsed: toml::Value = toml::from_str(&legacy).unwrap();
    assert!(parsed["tape"].get("max_age_days").is_none());
    assert!(parsed["tape"].get("record_emulator").is_none());
    let current = format!("{legacy}future = 'kept'\n");
    let text = with_tape(&current, tape, None, Some(keep))
        .unwrap()
        .unwrap();
    let parsed: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(parsed["tape"]["max_age_days"].as_integer(), Some(90));
    assert_eq!(parsed["tape"]["record_emulator"].as_bool(), Some(false));
    assert_eq!(parsed["tape"]["future"].as_str(), Some("kept"));
    assert!(with_tape(&text, tape, None, Some(keep)).unwrap().is_none());
    let moved = TapeWindow {
        margin_s: 300,
        ..tape
    };
    let text = with_tape(&text, moved, None, None).unwrap().unwrap();
    let parsed: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(parsed["tape"]["max_age_days"].as_integer(), Some(90));
    assert_eq!(parsed["tape"]["record_emulator"].as_bool(), Some(false));
}

/// The tuner-only key reaches `station.toml` only when the station reported the choice: an
/// older station rejects an unknown `[tape]` key and would stop starting.
#[test]
fn with_tape_writes_tuner_only_only_when_supported() {
    let tape = TapeWindow {
        margin_s: 180,
        long_position_min: 10,
    };
    let keep = |tuner_only| TapeKeep {
        max_age_days: 90,
        record_emulator: true,
        tuner_only,
        min_tail_s: None,
    };
    let old = with_tape("", tape, None, Some(keep(None)))
        .unwrap()
        .unwrap();
    let parsed: toml::Value = toml::from_str(&old).unwrap();
    assert!(parsed["tape"].get("tuner_only").is_none());
    let text = with_tape(&old, tape, None, Some(keep(Some(true))))
        .unwrap()
        .unwrap();
    let parsed: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(parsed["tape"]["tuner_only"].as_bool(), Some(true));
    assert!(
        with_tape(&text, tape, None, Some(keep(Some(true))))
            .unwrap()
            .is_none()
    );
    // A station that reports retention but not the tuner choice cannot apply a sent one.
    assert_eq!(
        tape_verdict(
            Some((tape, None, Some(keep(None)))),
            tape,
            None,
            Some(keep(Some(true)))
        ),
        TapeVerdict::Outdated
    );
    assert_eq!(
        tape_verdict(
            Some((tape, None, Some(keep(Some(true))))),
            tape,
            None,
            Some(keep(Some(true)))
        ),
        TapeVerdict::Applied
    );
}

/// Tail writes must omit unsupported keys and accept a newer station's unrequested fields.
#[test]
fn with_tape_min_tail_is_capability_gated_and_acknowledged() {
    let tape = TapeWindow {
        margin_s: 30,
        long_position_min: 10,
    };
    let old = TapeKeep {
        max_age_days: 90,
        record_emulator: true,
        tuner_only: None,
        min_tail_s: None,
    };
    let text = with_tape("", tape, None, Some(old)).unwrap().unwrap();
    let parsed: toml::Value = toml::from_str(&text).unwrap();
    assert!(parsed["tape"].get("min_tail_s").is_none());
    let wanted = TapeKeep {
        min_tail_s: Some(120),
        ..old
    };
    let text = with_tape(&text, tape, None, Some(wanted)).unwrap().unwrap();
    let parsed: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(parsed["tape"]["min_tail_s"].as_integer(), Some(120));
    assert!(
        with_tape(&text, tape, None, Some(wanted))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        tape_verdict(Some((tape, None, Some(old))), tape, None, Some(wanted)),
        TapeVerdict::Outdated
    );
    assert_eq!(
        tape_verdict(Some((tape, None, Some(wanted))), tape, None, Some(old)),
        TapeVerdict::Applied
    );
    assert_eq!(
        tape_verdict(Some((tape, None, Some(wanted))), tape, None, Some(wanted)),
        TapeVerdict::Applied
    );
}

/// Missing retention support must fail immediately only when retention choices were sent.
#[test]
fn tape_verdict_outdated_without_keep() {
    let tape = TapeWindow {
        margin_s: 180,
        long_position_min: 10,
    };
    let keep = TapeKeep {
        max_age_days: 90,
        record_emulator: false,
        tuner_only: None,
        min_tail_s: None,
    };
    assert_eq!(
        tape_verdict(None, tape, None, Some(keep)),
        TapeVerdict::Outdated
    );
    assert_eq!(
        tape_verdict(Some((tape, None, None)), tape, None, Some(keep)),
        TapeVerdict::Outdated
    );
    assert_eq!(
        tape_verdict(Some((tape, None, None)), tape, None, None),
        TapeVerdict::Applied
    );
    assert_eq!(
        tape_verdict(Some((tape, None, Some(keep))), tape, None, Some(keep)),
        TapeVerdict::Applied
    );
    let old = TapeKeep {
        max_age_days: 365,
        record_emulator: true,
        tuner_only: None,
        min_tail_s: None,
    };
    assert_eq!(
        tape_verdict(Some((tape, None, Some(old))), tape, None, Some(keep)),
        TapeVerdict::Pending
    );
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
core_endpoint_override=no
"
    ));
    assert!(!helper_is_current(
        "release_update=yes
remove_station=no
"
    ));
    assert!(!helper_is_current("remove_station=yes\n"));
}

/// Treating capability=no as a stale helper would reinstall it on every old-station refresh.
#[test]
fn helper_capability_reading_is_current_for_both_binary_generations() {
    let base = "bot_return=yes\nremove_station=yes\nremoval_guard=yes\nbot_settings_merge=yes\n";
    for capability in ["yes", "no"] {
        assert!(helper_is_current(&format!(
            "{base}core_endpoint_override={capability}\n"
        )));
    }
    assert!(!helper_is_current(base));
    assert!(!helper_is_current(&format!(
        "{base}core_endpoint_override=unknown\n"
    )));
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

/// A station that reports no switches after they were sent never will: waiting out the deadline
/// only delays the "update the station" error.
#[test]
fn tape_switches_sent_to_a_pre_switch_station_fail_at_once() {
    let tape = TapeWindow {
        margin_s: 60,
        long_position_min: 30,
    };
    let on = CatchupSwitches {
        auto: true,
        archives: true,
    };
    assert_eq!(
        tape_verdict(Some((tape, None, None)), tape, Some(on), None),
        TapeVerdict::Outdated
    );
    assert_eq!(
        tape_verdict(None, tape, Some(on), None),
        TapeVerdict::Outdated
    );
    assert_eq!(
        tape_verdict(None, tape, None, None),
        TapeVerdict::Unreported
    );
    assert_eq!(
        tape_verdict(Some((tape, Some(on), None)), tape, Some(on), None),
        TapeVerdict::Applied
    );
    assert_eq!(
        tape_verdict(Some((tape, None, None)), tape, None, None),
        TapeVerdict::Applied
    );
    let old = TapeWindow {
        margin_s: 30,
        ..tape
    };
    assert_eq!(
        tape_verdict(Some((old, Some(on), None)), tape, Some(on), None),
        TapeVerdict::Pending
    );
}
