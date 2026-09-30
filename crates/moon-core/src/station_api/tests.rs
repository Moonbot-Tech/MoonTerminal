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
    }));
    let text = serde_json::to_string(&reply).unwrap();
    assert_eq!(serde_json::from_str::<Reply>(&text).unwrap(), reply);
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
