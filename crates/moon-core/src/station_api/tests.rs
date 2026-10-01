use super::*;

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
            base: Access::default(),
            access: Access {
                authorized_chat_ids: vec![7],
                owner_chat_id: Some(7),
                chat_access: Vec::new(),
            },
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
    }));
    let text = serde_json::to_string(&reply).unwrap();
    assert_eq!(serde_json::from_str::<Reply>(&text).unwrap(), reply);
    // A station older than the window answers without it.
    let old = r#"{"ok":{"status":{"station_version":"0.1.0","cores_ready":1,"cores_total":1,"bot":null}}}"#;
    let Reply::Ok(Answer::Status(old)) = serde_json::from_str::<Reply>(old).unwrap() else {
        panic!("not a status");
    };
    assert_eq!((old.tape, old.host, old.last_update), (None, None, None));
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
    };
    access.apply_to(&mut telegram);
    assert_eq!(Access::of(&telegram), access);
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
