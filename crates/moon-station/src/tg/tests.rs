use moon_core::config::telegram_access::TelegramChatAccess;

use super::*;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moon-station-tg-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(PAIRING_FILE)
}

/// A pairing survives a restart as it was saved: chats, owner and the viewers' grants.
#[test]
fn a_saved_pairing_reads_back_whole() {
    let path = scratch("roundtrip");
    assert!(
        load_pairing(&path).unwrap().authorized_chat_ids.is_empty(),
        "no file yet is no pairing, not an error"
    );
    let pairing = Access {
        authorized_chat_ids: vec![7, 9],
        owner_chat_id: Some(7),
        chat_access: vec![TelegramChatAccess {
            chat_id: 9,
            name: "viewer".into(),
            core_uids: vec![3],
        }],
        ..Access::default()
    };
    write_pairing(&path, &pairing).unwrap();
    let back = load_pairing(&path).unwrap();
    assert_eq!(back.authorized_chat_ids, vec![7, 9]);
    assert_eq!(back.owner_chat_id, Some(7));
    assert_eq!(back.chat_access, pairing.chat_access);
    assert!(
        !path.with_extension("json.new").exists(),
        "the staging file is renamed, not left behind"
    );
}

/// A damaged file is an error, never an empty pairing: an empty one would be overwritten by the
/// next pairing and every grant in the file lost.
#[test]
fn a_damaged_pairing_is_refused() {
    let path = scratch("damaged");
    std::fs::write(&path, "{ not json").unwrap();
    assert!(load_pairing(&path).is_err());
}

/// A field this binary does not know — written by a newer one before a rollback — is skipped, and
/// the pairing it knows still loads: the bot must not stay off after a rollback.
#[test]
fn a_newer_field_does_not_stop_the_bot() {
    let path = scratch("newer");
    std::fs::write(
        &path,
        r#"{"authorized_chat_ids": [7], "owner_chat_id": 7, "since_ms": 1}"#,
    )
    .unwrap();
    let pairing = load_pairing(&path).unwrap();
    assert_eq!(pairing.authorized_chat_ids, vec![7]);
    assert_eq!(pairing.owner_chat_id, Some(7));
}

/// A change keeps what it leaves out: the bot's settings, and the zone only once one was pushed —
/// until then `station.toml`'s stays the fallback and is not written down.
#[test]
fn a_change_keeps_what_it_leaves_out() {
    use moon_core::config::telegram_menu::{BotSettings, ReportView};
    let bot = BotSettings {
        report_view: ReportView::Days,
        ..BotSettings::default()
    };
    let current = Access {
        authorized_chat_ids: vec![7],
        owner_chat_id: Some(7),
        bot: Some(bot.clone()),
        zone: Some("Europe/Moscow".into()),
        ..Access::default()
    };
    // An older terminal: no bot settings, no zone, base without them.
    let old_base = Access {
        bot: None,
        zone: None,
        ..current.clone()
    };
    let change = Access {
        authorized_chat_ids: vec![7, 9],
        owner_chat_id: Some(7),
        ..Access::default()
    };
    let saved = super::plan_access(&current, None, &old_base, change.clone()).unwrap();
    assert_eq!(saved.authorized_chat_ids, vec![7, 9]);
    assert_eq!(saved.bot, Some(bot.clone()));
    assert_eq!(
        saved.zone, None,
        "the toml zone is not frozen into the file"
    );
    let saved = super::plan_access(&current, Some("Asia/Tokyo"), &old_base, change).unwrap();
    assert_eq!(saved.zone.as_deref(), Some("Asia/Tokyo"));
}

/// A base whose chats or bot settings moved since is refused; the zone never counts, and a zone
/// this build cannot read is refused rather than saved.
#[test]
fn a_stale_base_or_a_bad_zone_is_refused() {
    use moon_core::config::telegram_menu::{BotSettings, ReportBasis};
    let current = Access {
        authorized_chat_ids: vec![7],
        owner_chat_id: Some(7),
        bot: Some(BotSettings::default()),
        zone: Some("UTC".into()),
        ..Access::default()
    };
    let moved_bot = Access {
        bot: Some(BotSettings {
            period_basis: ReportBasis::Open,
            ..BotSettings::default()
        }),
        ..current.clone()
    };
    assert!(super::plan_access(&current, None, &moved_bot, current.clone()).is_err());
    let moved_chats = Access {
        authorized_chat_ids: vec![9],
        owner_chat_id: Some(9),
        ..current.clone()
    };
    assert!(super::plan_access(&current, None, &moved_chats, current.clone()).is_err());
    let other_zone = Access {
        zone: Some("Asia/Tokyo".into()),
        ..current.clone()
    };
    assert!(super::plan_access(&current, None, &other_zone, current.clone()).is_ok());
    let bad_zone = Access {
        zone: Some("Mars/Base".into()),
        ..current.clone()
    };
    assert!(super::plan_access(&current, None, &current, bad_zone).is_err());
}

/// The pushed zone reads back from the file; one this build cannot read is no zone.
#[test]
fn the_pushed_zone_reads_back() {
    let path = scratch("zone");
    let pairing = Access {
        zone: Some("Europe/Berlin".into()),
        bot: Some(Default::default()),
        ..Access::default()
    };
    write_pairing(&path, &pairing).unwrap();
    let back = load_pairing(&path).unwrap();
    assert_eq!(back, pairing);
    assert_eq!(
        super::pushed_zone(&back).map(|(name, _)| name),
        Some("Europe/Berlin".to_owned())
    );
    let unreadable = Access {
        zone: Some("Mars/Base".into()),
        ..Access::default()
    };
    assert!(super::pushed_zone(&unreadable).is_none());
}

/// Chats' notifications travel with a change but are never written into `telegram.json`.
#[test]
fn notifications_stay_out_of_the_pairing_file() {
    use moon_core::station_api::ChatNotifyRow;
    let current = Access {
        authorized_chat_ids: vec![7],
        owner_chat_id: Some(7),
        bot: Some(Default::default()),
        ..Access::default()
    };
    let change = Access {
        notify: Some(std::collections::BTreeMap::from([(
            7,
            ChatNotifyRow::default(),
        )])),
        ..current.clone()
    };
    let saved = super::plan_access(&current, None, &current, change).unwrap();
    assert_eq!(saved.notify, None);
}

/// Core groups ride a change on their own: sent, they replace the station's whole and are kept in
/// the terminal's shape; left out — any change that is not the groups' — they stay. They never
/// count against the base, so sending them does not clash with an edit of the menu.
#[test]
fn groups_replace_the_stations_whole_or_stay() {
    use moon_core::config::CoreGroup;
    let group = |name: &str, cores: &[u64]| CoreGroup {
        name: name.into(),
        cores: cores.to_vec(),
    };
    let current = Access {
        authorized_chat_ids: vec![7],
        owner_chat_id: Some(7),
        groups: Some(vec![group("main", &[1, 2])]),
        ..Access::default()
    };
    let base = Access {
        groups: Some(vec![group("other", &[3])]),
        ..current.clone()
    };
    let kept = super::plan_access(
        &current,
        None,
        &base,
        Access {
            groups: None,
            ..current.clone()
        },
    )
    .unwrap();
    assert_eq!(
        kept.groups, current.groups,
        "a change without groups keeps them"
    );
    let sent = super::plan_access(
        &current,
        None,
        &base,
        Access {
            groups: Some(vec![group("  AAA ", &[4, 4]), group("margo", &[])]),
            ..current.clone()
        },
    )
    .unwrap();
    assert_eq!(
        sent.groups,
        Some(vec![group("AAA", &[4])]),
        "sent groups replace the set, trimmed, deduplicated, empty ones dropped"
    );
}
