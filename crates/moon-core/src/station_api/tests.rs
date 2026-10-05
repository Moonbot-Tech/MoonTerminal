use super::*;

/// Losing layout in Access storage or equality would discard station edits or accept stale ones.
#[test]
fn message_layout_rides_access_and_participates_in_base_holds() {
    let access: Access = serde_json::from_value(serde_json::json!({"bot": {"message_layout": {"card": {"lines": [["core"], ["coin", "prices"]], "core_hashtag": false}, "report": {"columns": ["volume", "average"], "total": "top", "separation": "gap_band"}}}})).unwrap();
    let wire = serde_json::to_string(&access).unwrap();
    let back: Access = serde_json::from_str(&wire).unwrap();
    let mut telegram = TelegramConfig::default();
    back.apply_to(&mut telegram);
    assert_eq!(
        telegram.bot.message_layout.card.lines,
        vec![
            vec![crate::config::CardField::Core],
            vec![
                crate::config::CardField::Coin,
                crate::config::CardField::Prices
            ]
        ]
    );
    assert!(!telegram.bot.message_layout.card.core_hashtag);
    assert_eq!(
        telegram.bot.message_layout.report.columns,
        vec![
            crate::config::ReportColumn::Volume,
            crate::config::ReportColumn::Average
        ]
    );
    assert_eq!(
        telegram.bot.message_layout.report.total,
        crate::config::TotalPlace::Top
    );
    assert_eq!(
        telegram.bot.message_layout.report.separation,
        crate::config::TotalSeparation::GapBand
    );
    let current = Access::of(&telegram);
    assert!(back.base_holds(&current));
    telegram.bot.message_layout.card.coin_hashtag = false;
    assert!(!back.base_holds(&Access::of(&telegram)));
}

/// Frozen TESTKEY V1 export: master 0x11, MAC 0x22, endpoint 198.51.100.42:4321.
const SYNTHETIC_KEY: &str = "sX85BQAAAAD4HMdln7gLXlN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AArwH8gJLfmM/47yuKci+sFrrtNibJShbRnc1HGycnqLRazhICIMdoPAhGryNcv1KZClUCEhH6mRG/Np81EodJlA=="; // gitleaks:allow

/// Dropping the domain separator or trimming changes fingerprints across station and terminal.
#[test]
fn core_fingerprints_are_domain_separated_trimmed_and_secret_free() {
    assert_eq!(key_fingerprint("synthetic-core-key"), "5e7efb7e11148007");
    assert_eq!(
        key_fingerprint(" \nsynthetic-core-key\t"),
        "5e7efb7e11148007"
    );
    assert_ne!(
        key_fingerprint("synthetic-core-key"),
        key_fingerprint("other-synthetic-key")
    );
    assert!(!key_fingerprint("synthetic-core-key").contains("synthetic-core-key"));
}

/// Displaying fallback or losing the port would match station cores to the wrong endpoint.
#[test]
fn listed_core_addresses_decode_synthetic_keys() {
    assert_eq!(
        core_address(SYNTHETIC_KEY, "").as_deref(),
        Some("198.51.100.42:4321")
    );
    assert_eq!(core_address("not-a-key", ""), None);
}

/// Falling back to the key or resolving DNS would match an overridden core to the wrong station row.
#[test]
fn station_addresses_use_the_feed_target_without_dns_resolution() {
    for (override_text, expected) in [
        ("203.0.113.9:5020", "203.0.113.9:5020"),
        (":5020", "198.51.100.42:5020"),
        ("Core.Example.Invalid", "core.example.invalid:4321"),
        ("[2001:db8::7]:5020", "[2001:db8::7]:5020"),
    ] {
        assert_eq!(
            core_address(SYNTHETIC_KEY, override_text).as_deref(),
            Some(expected)
        );
    }
    assert_eq!(core_address(SYNTHETIC_KEY, "host:0"), None);
    assert_eq!(core_address("not-a-key", "core.example.invalid:5020"), None);
}

/// Making the override mandatory would reject old listings; losing empty Some would hide new support.
#[test]
fn override_listing_capability_is_optional_and_preserves_typed_text() {
    let old = r#"{"uid":7,"name":"Fixture","address":"198.51.100.42:4321","key_fp":"fixture"}"#;
    let mut core: ListedCore = serde_json::from_str(old).unwrap();
    assert_eq!(core.endpoint_override, None);
    assert_eq!(serde_json::to_string(&core).unwrap(), old);
    for text in ["", "Core.Example.Invalid:5020"] {
        core.endpoint_override = Some(text.into());
        let back: ListedCore =
            serde_json::from_str(&serde_json::to_string(&core).unwrap()).unwrap();
        assert_eq!(back.endpoint_override.as_deref(), Some(text));
    }
}

fn chat(chat_id: i64, core_uids: &[u64]) -> TelegramChatAccess {
    TelegramChatAccess {
        chat_id,
        name: format!("chat {chat_id}"),
        core_uids: core_uids.to_vec(),
    }
}

/// The command names are the wire's words (STATION.md §4.5), not Rust's.
#[test]
fn requests_carry_the_documented_command_names() {
    let text = serde_json::to_string(&Request::PairIssue).unwrap();
    assert_eq!(text, r#"{"cmd":"pair.issue"}"#);
    let set: Request = serde_json::from_str(
        r#"{"cmd":"access.set","base":{},"access":{"authorized_chat_ids":[7],"owner_chat_id":7}}"#,
    )
    .unwrap();
    assert_eq!(
        set,
        Request::AccessSet {
            base: Box::default(),
            access: Box::new(Access {
                authorized_chat_ids: vec![7],
                owner_chat_id: Some(7),
                ..Access::default()
            }),
        }
    );
}

/// A status with a bot survives the round trip whole: the client reads what the station meant.
#[test]
fn a_status_reads_back_whole() {
    let reply = Reply::Ok(Answer::Status(Status {
        station_version: "0.1.0".into(),
        cores_ready: 26,
        cores_total: 27,
        bot: Some(BotStatus {
            status: TelegramStatus::Paired { chat_count: 2 },
            mini_app_on: true,
            mini_app: MiniAppStatus::Tunneling {
                port: 1,
                url: "https://x.trycloudflare.com".into(),
            },
            pairing: Some(PairingCode {
                code: "VWG8MH".into(),
                expires_in_s: 540,
            }),
        }),
        tape: Some(TapeWindow {
            margin_s: 180,
            long_position_min: 10,
        }),
        host: Some(Box::new(Host {
            uptime_s: 3_725,
            cpu: vec![CpuWindow {
                minutes: 60,
                station_avg_permille: 123,
                station_peak_permille: 870,
                machine_avg_permille: 150,
                machine_peak_permille: 1000,
            }],
            memory: Some(Memory {
                rss_bytes: 420 << 20,
                rss_peak_bytes: 520 << 20,
                available_bytes: 300 << 20,
                total_bytes: 955 << 20,
            }),
            disk: Some(Disk {
                free_bytes: 15 << 30,
                total_bytes: 23 << 30,
            }),
            files: vec![DataFile {
                name: "reports.sqlite".into(),
                bytes: 609 << 20,
            }],
        })),
        last_update: Some("2026-09-30T14:02Z health=ok".into()),
        auto_update: Some(false),
        core_uid_high_water: Some(12),
        cores: Some(vec![ListedCore {
            endpoint_override: None,
            uid: 3,
            name: "Core A".into(),
            address: Some("198.51.100.42:4321".into()),
            key_fp: Some("9f2c0123456789ab".into()),
        }]),
    }));
    let text = serde_json::to_string(&reply).unwrap();
    assert_eq!(serde_json::from_str::<Reply>(&text).unwrap(), reply);
    // A station older than the window answers without it.
    let old = r#"{"ok":{"status":{"station_version":"0.1.0","cores_ready":1,"cores_total":1,"bot":null}}}"#;
    let Reply::Ok(Answer::Status(old)) = serde_json::from_str::<Reply>(old).unwrap() else {
        panic!("not a status");
    };
    assert_eq!((old.tape, old.host, old.last_update), (None, None, None));
    assert_eq!(old.cores, None);
    assert_eq!(old.core_uid_high_water, None);
    assert_eq!(
        old.auto_update, None,
        "a station older than the switch reads as unknown, not as on or off"
    );
    let refused: Reply = serde_json::from_str(r#"{"err":"no bot on this station"}"#).unwrap();
    assert_eq!(refused, Reply::Err("no bot on this station".into()));
}

#[test]
fn a_pairing_a_bot_can_run_passes() {
    let access = Access {
        authorized_chat_ids: vec![7, 9],
        owner_chat_id: Some(7),
        chat_access: vec![chat(9, &[3])],
        ..Access::default()
    };
    assert_eq!(access.check(), Ok(()));
    assert_eq!(
        Access::default().check(),
        Ok(()),
        "no chat is a pairing too"
    );
}

#[test]
fn a_pairing_a_bot_cannot_run_is_refused() {
    let twice = Access {
        authorized_chat_ids: vec![7, 7],
        ..Access::default()
    };
    assert!(twice.check().is_err());
    let stray_owner = Access {
        authorized_chat_ids: vec![7],
        owner_chat_id: Some(9),
        ..Access::default()
    };
    assert!(stray_owner.check().is_err());
    let stray_profile = Access {
        authorized_chat_ids: vec![7],
        owner_chat_id: Some(7),
        chat_access: vec![chat(9, &[1])],
        ..Access::default()
    };
    assert!(stray_profile.check().is_err());
    let no_owner = Access {
        authorized_chat_ids: vec![7, 9],
        ..Access::default()
    };
    assert!(
        no_owner.check().is_err(),
        "paired chats need an explicit owner"
    );
}

/// A saved pairing from before the owner existed gets its first chat as the explicit owner.
#[test]
fn a_legacy_pairing_gets_its_first_chat_as_owner() {
    let mut legacy: Access = serde_json::from_str(r#"{"authorized_chat_ids":[7,9]}"#).unwrap();
    assert!(legacy.adopt_legacy_owner());
    assert_eq!(legacy.owner_chat_id, Some(7));
    assert_eq!(legacy.check(), Ok(()));
    assert!(!legacy.adopt_legacy_owner(), "an explicit owner stays");
    let mut empty = Access::default();
    assert!(!empty.adopt_legacy_owner());
    assert_eq!(empty.owner_chat_id, None);
}

/// Access goes into a configuration without touching its token or its switches.
#[test]
fn access_moves_in_and_out_of_a_configuration() {
    let mut telegram = TelegramConfig {
        token: crate::config::Secret::new("t".to_string()),
        mini_app_enabled: true,
        ..TelegramConfig::default()
    };
    let access = Access {
        authorized_chat_ids: vec![7],
        owner_chat_id: Some(7),
        chat_access: vec![chat(7, &[])],
        ..Access::default()
    };
    access.apply_to(&mut telegram);
    assert!(Access::of(&telegram).same_chats(&access));
    // Without the bot's settings the configuration keeps its own; `of` always carries them.
    assert_eq!(Access::of(&telegram).bot, Some(telegram.bot.clone()));
    assert_eq!(telegram.token.expose(), "t");
    assert!(telegram.mini_app_enabled);
}

/// The pull's requests carry their names on the wire, and a trace line survives the trip both
/// ways.
#[test]
fn pull_requests_and_trace_lines_round_trip() {
    use crate::feed::report_traces::{ArchivedLineKind, ArchivedOrderTrace};
    let fetch = Request::TapeFetch {
        items: vec![TapeWant {
            exchange: "binf:00000000".into(),
            market: "ACEUSDT".into(),
            spans: vec![(1_000, 2_000)],
        }],
    };
    let json = serde_json::to_value(&fetch).unwrap();
    assert_eq!(json["cmd"], "tape.fetch");
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), fetch);
    let traces = Request::TracesFetch {
        core_uid: 7,
        report_uids: vec![-5, 9],
    };
    assert_eq!(
        serde_json::to_value(&traces).unwrap()["cmd"],
        "traces.fetch"
    );
    let archived = ArchivedOrderTrace {
        own: false,
        kind: ArchivedLineKind::Exit,
        stop_price: Some(1.5),
        stop_time_ms: None,
        points: vec![(1_000.0, 2.25), (2_000.0, 2.5)],
    };
    let wire: TraceLine = (&archived).into();
    let back: ArchivedOrderTrace =
        serde_json::from_value::<TraceLine>(serde_json::to_value(&wire).unwrap())
            .unwrap()
            .into();
    assert_eq!(back, archived);
}

/// A change still holds when only the zone moved; it does not when the chats or — read by the
/// client — the bot's settings moved. A client that never read the settings is not held to them.
#[test]
fn a_base_holds_on_chats_and_the_settings_it_read() {
    use crate::config::telegram_menu::{BotSettings, ReportView};
    let current = Access {
        authorized_chat_ids: vec![7],
        owner_chat_id: Some(7),
        bot: Some(BotSettings::default()),
        zone: Some("UTC".into()),
        ..Access::default()
    };
    let other_zone = Access {
        zone: Some("Asia/Tokyo".into()),
        ..current.clone()
    };
    assert!(other_zone.base_holds(&current));
    let unread_settings = Access {
        bot: None,
        ..current.clone()
    };
    assert!(unread_settings.base_holds(&Access {
        bot: Some(BotSettings {
            report_view: ReportView::Cores,
            ..BotSettings::default()
        }),
        ..current.clone()
    }));
    let moved_settings = Access {
        bot: Some(BotSettings {
            report_view: ReportView::Cores,
            ..BotSettings::default()
        }),
        ..current.clone()
    };
    assert!(!moved_settings.base_holds(&current));
    let moved_chats = Access {
        chat_access: vec![chat(7, &[1])],
        ..current.clone()
    };
    assert!(!moved_chats.base_holds(&current));
}

/// On the wire and on disk an access without the bot's settings and zone is exactly the old form,
/// and the old form reads back without them.
#[test]
fn the_new_parts_are_optional_on_the_wire() {
    let old_json = r#"{"authorized_chat_ids":[7],"owner_chat_id":7,"chat_access":[]}"#;
    let old: Access = serde_json::from_str(old_json).unwrap();
    assert_eq!(old.bot, None);
    assert_eq!(old.zone, None);
    assert_eq!(serde_json::to_string(&old).unwrap(), old_json);
    let full = Access {
        bot: Some(Default::default()),
        zone: Some("UTC".into()),
        ..old.clone()
    };
    let back: Access = serde_json::from_str(&serde_json::to_string(&full).unwrap()).unwrap();
    assert_eq!(back, full);
    let request = Request::AccessSet {
        base: Box::new(old.clone()),
        access: Box::new(full),
    };
    let back: Request = serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
    assert_eq!(back, request);
}

/// Chats' notifications are optional on the wire and survive the trip with their revisions; a
/// change of them alone still holds against its base.
#[test]
fn chats_notifications_ride_the_access() {
    let mut row = ChatNotifyRow::default();
    row.settings.down.on = true;
    row.revision = 4;
    let access = Access {
        authorized_chat_ids: vec![7],
        owner_chat_id: Some(7),
        notify: Some(std::collections::BTreeMap::from([(7, row)])),
        ..Access::default()
    };
    let back: Access = serde_json::from_str(&serde_json::to_string(&access).unwrap()).unwrap();
    assert_eq!(back, access);
    let read = Access {
        notify: None,
        ..access.clone()
    };
    assert!(read.base_holds(&access));
    assert!(!serde_json::to_string(&read).unwrap().contains("notify"));
}

/// A change of chats' notifications reaches the station: the chat ids are a JSON object's keys —
/// strings on the wire — and an `access.set` is an internally tagged request, which serde buffers
/// before it reads the fields. A group's id is negative.
#[test]
fn chats_notifications_ride_an_access_set() {
    let access = Access {
        authorized_chat_ids: vec![230057918, -1001234567890],
        owner_chat_id: Some(230057918),
        notify: Some(std::collections::BTreeMap::from([
            (230057918, ChatNotifyRow::default()),
            (-1001234567890, ChatNotifyRow::default()),
        ])),
        ..Access::default()
    };
    let request = Request::AccessSet {
        base: Box::new(access.clone()),
        access: Box::new(access),
    };
    let wire = serde_json::to_string(&request).unwrap();
    assert!(wire.contains(r#""230057918":"#), "{wire}");
    let back: Request = serde_json::from_str(&wire).unwrap();
    assert_eq!(back, request);
}
