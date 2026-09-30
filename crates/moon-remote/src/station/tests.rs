use super::*;

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
    let merged: toml::Value = toml::from_str(&keep_telegram(new, Some(current)).unwrap()).unwrap();
    assert_eq!(merged["core"][0]["uid"].as_integer(), Some(3));
    assert_eq!(merged["core"].as_array().unwrap().len(), 1);
    assert_eq!(merged["telegram"]["mini_app"].as_bool(), Some(true));
    assert_eq!(keep_telegram(new, None).unwrap(), new);
    let bare = "[[core]]\nuid = 9\nname = \"Old\"\n";
    assert!(
        toml::from_str::<toml::Value>(&keep_telegram(new, Some(bare)).unwrap())
            .unwrap()
            .get("telegram")
            .is_none()
    );
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

/// A helper older than the control API prints no `api=`: it is replaced before any write.
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
    assert!(helper_is_current(
        "active=active
config=yes
creds=core-3
token=no
pairing=no
valuation=no
api=no
"
    ));
}
