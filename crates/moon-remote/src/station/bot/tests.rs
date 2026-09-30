use moon_core::config::telegram_access::TelegramChatAccess;
use moon_core::station_api::PairingCode;
use moon_core::telegram::runtime::mini_app::MiniAppStatus;

use super::*;

/// Removing before reading destroys the only bot; a failed read must run no removal.
#[test]
fn return_read_failure_never_removes_the_bot() {
    let removed = std::cell::Cell::new(false);
    let result = read_then_remove(
        true,
        || Err(BotReturnError::ReadFailed.into()),
        &mut |_| {},
        |_, _| {
            removed.set(true);
            Ok(())
        },
    );
    assert!(result.is_err());
    assert!(!removed.get());
}

/// A premature return starts a second poller before server removal completes.
#[test]
fn returning_reads_before_removal_and_waits_for_it() {
    let steps = std::cell::RefCell::new(Vec::new());
    let returned = read_then_remove(
        true,
        || {
            steps.borrow_mut().push("read");
            Ok(ReturnedBot {
                token: Secret::new("synthetic-token"),
                access: Access::default(),
            })
        },
        &mut |_| {},
        |_, _| {
            steps.borrow_mut().push("remove");
            Ok(())
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(*steps.borrow(), ["read", "remove"]);
    assert_eq!(returned.token.expose(), "synthetic-token");
}

/// Requiring a new helper for a second local bot incorrectly blocks simple removal.
#[test]
fn local_bot_skips_readback_but_removes_server_bot() {
    let removed = std::cell::Cell::new(false);
    let returned = read_then_remove(
        false,
        || panic!("must not export another bot"),
        &mut |_| {},
        |_, _| {
            removed.set(true);
            Ok(())
        },
    )
    .unwrap();
    assert!(returned.is_none());
    assert!(removed.get());
}

/// script::checked here would expose a token echoed on stdout or stderr in the UI error.
#[test]
fn credential_read_errors_never_expose_output() {
    for status in [Some(1), None] {
        let error = decode_token(crate::ssh::SecretOutput {
            status,
            stdout: zeroize::Zeroizing::new(b"synthetic-secret-token".to_vec()),
            stderr: zeroize::Zeroizing::new(b"synthetic-secret-token".to_vec()),
        })
        .unwrap_err();
        assert!(error.downcast_ref::<BotReturnError>().is_some());
        assert!(!format!("{error:#}").contains("synthetic-secret-token"));
    }
}

/// Empty or malformed output must not erase the sole server credential.
#[test]
fn invalid_credentials_fail_closed() {
    for text in ["", "\n", "first\nsecond"] {
        assert!(
            decode_token(crate::ssh::SecretOutput {
                status: Some(0),
                stdout: zeroize::Zeroizing::new(text.as_bytes().to_vec()),
                stderr: zeroize::Zeroizing::new(Vec::new()),
            })
            .is_err()
        );
    }
}

/// Dropping the snapshot on a failed removal loses the bot if the credential was already deleted.
#[test]
fn removal_failure_retains_recovered_credentials_without_a_success() {
    let mut retained = None;
    let result = read_then_remove(
        true,
        || {
            Ok(ReturnedBot {
                token: Secret::new("synthetic-retained"),
                access: Access::default(),
            })
        },
        &mut |returned| retained = Some(returned),
        |_, _| Err(anyhow::anyhow!("synthetic restart failure")),
    );
    assert!(result.is_err());
    assert_eq!(retained.unwrap().token.expose(), "synthetic-retained");
}

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
