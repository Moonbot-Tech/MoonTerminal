use super::*;

/// Dropping credentials before a failed config commit strands the old referenced core on restart.
#[test]
fn a_failed_config_write_never_drops_credentials() {
    let dropped = std::cell::Cell::new(false);
    let result = commit_cores_config(
        || anyhow::bail!("synthetic CAS or SSH failure"),
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

/// Reversing the callbacks makes cleanup observe the old config; a cleanup error must leave
/// the new references committed and the extra unused credential harmless.
#[test]
fn credential_cleanup_failure_keeps_the_new_config() {
    let referenced = std::cell::Cell::new(9);
    let result = commit_cores_config(
        || {
            referenced.set(3);
            Ok(true)
        },
        || {
            assert_eq!(
                referenced.get(),
                3,
                "the stale core must be unreferenced first"
            );
            anyhow::bail!("synthetic credential cleanup failure")
        },
    );
    assert!(result.is_err());
    assert_eq!(referenced.get(), 3);
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
    let text = station_toml(
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
        toml::from_str::<toml::Value>(&station_toml(&cores, None).unwrap())
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
    let new = "[[core]]\nuid = 3\nname = \"A\"\n";
    let current =
        "[[core]]\nuid = 9\nname = \"Old\"\n\n[telegram]\nmini_app = true\nzone = \"UTC\"\n";
    let merged: toml::Value =
        toml::from_str(&keep_server_sections(new, Some(current)).unwrap()).unwrap();
    assert_eq!(merged["core"][0]["uid"].as_integer(), Some(3));
    assert_eq!(merged["core"].as_array().unwrap().len(), 1);
    assert_eq!(merged["telegram"]["mini_app"].as_bool(), Some(true));
    assert_eq!(keep_server_sections(new, None).unwrap(), new);
    let bare = "[[core]]\nuid = 9\nname = \"Old\"\n";
    assert!(
        toml::from_str::<toml::Value>(&keep_server_sections(new, Some(bare)).unwrap())
            .unwrap()
            .get("telegram")
            .is_none()
    );
}

/// A cores push never replaces the station's window: the terminal's only starts a station that
/// has none. The window is set by hand (`push_tape`), never behind the user's back.
#[test]
fn a_cores_push_never_replaces_the_stations_window() {
    let current =
        "[[core]]\nuid = 9\nname = \"Old\"\n\n[tape]\nmargin_s = 300\nlong_position_min = 20\n";
    let with_window =
        "[[core]]\nuid = 3\nname = \"A\"\n\n[tape]\nmargin_s = 180\nlong_position_min = 10\n";
    let merged: toml::Value =
        toml::from_str(&keep_server_sections(with_window, Some(current)).unwrap()).unwrap();
    assert_eq!(merged["tape"]["margin_s"].as_integer(), Some(300));
    assert_eq!(merged["tape"]["long_position_min"].as_integer(), Some(20));
    assert_eq!(merged["core"][0]["uid"].as_integer(), Some(3));

    let bare_server = "[[core]]\nuid = 9\nname = \"Old\"\n";
    let merged: toml::Value =
        toml::from_str(&keep_server_sections(with_window, Some(bare_server)).unwrap()).unwrap();
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

    let new = "[[core]]\nuid = 4\nname = \"B\"\n";
    let merged: toml::Value =
        toml::from_str(&keep_server_sections(new, Some(&off)).unwrap()).unwrap();
    assert_eq!(merged["update"]["auto"].as_bool(), Some(false));
}
