use moon_core::config::telegram_access::TelegramChatAccess;
use moon_core::station_api::PairingCode;
use moon_core::telegram::runtime::mini_app::MiniAppStatus;

use super::*;

fn running(status: TelegramStatus, code: Option<&str>) -> BotState {
    BotState {
        has_token: true,
        bot: Some(BotStatus {
            status,
            mini_app_on: false,
            mini_app: MiniAppStatus::Stopped,
            pairing: code.map(|code| PairingCode {
                code: code.into(),
                expires_in_s: 600,
            }),
        }),
        pairing_until: code.map(|_| Instant::now() + Duration::from_secs(600)),
        ..BotState::default()
    }
}

#[test]
fn an_unpaired_bot_polls_and_offers_its_code() {
    let state = running(TelegramStatus::Unpaired, Some("482913"));
    assert!(state.polling() && !state.paired());
    assert_eq!(state.pairing_code(), Some("482913"));
}

#[test]
fn a_paired_bot_polls() {
    let state = running(TelegramStatus::Paired { chat_count: 1 }, None);
    assert!(state.paired() && state.polling());
    assert_eq!(state.pairing_code(), None);
}

#[test]
fn a_starting_or_conflicting_bot_does_not_poll() {
    assert!(!running(TelegramStatus::Starting, None).polling());
    assert!(!running(TelegramStatus::Conflict, None).polling());
    let stopped = BotState {
        stopped: true,
        ..BotState::default()
    };
    assert!(!stopped.polling() && stopped.summary().contains("stopped"));
}

/// A code past its time is not offered: the bot would refuse it.
#[test]
fn an_expired_code_is_not_offered() {
    let mut state = running(TelegramStatus::Unpaired, Some("482913"));
    state.pairing_until = Some(Instant::now() - Duration::from_secs(1));
    assert_eq!(state.pairing_code(), None);
}

#[test]
fn the_pairing_file_is_what_the_station_reads() {
    let pairing = Access {
        authorized_chat_ids: vec![42],
        owner_chat_id: Some(42),
        chat_access: vec![TelegramChatAccess {
            chat_id: 42,
            name: "me".into(),
            core_uids: vec![1, 3],
        }],
    };
    let json: serde_json::Value = serde_json::to_value(&pairing).unwrap();
    assert_eq!(json["authorized_chat_ids"][0], 42);
    assert_eq!(json["owner_chat_id"], 42);
    assert_eq!(json["chat_access"][0]["core_uids"][1], 3);
}
