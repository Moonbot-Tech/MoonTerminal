use moon_core::config::telegram_access::TelegramChatAccess;
use moon_core::station_api::PairingCode;
use moon_core::telegram::runtime::mini_app::MiniAppStatus;

use super::*;

/// Keeping a pre-merge helper would leave transfer's direct file write destructive after upgrade.
#[test]
fn transfer_refreshes_helpers_that_cannot_preserve_bot_settings() {
    let old = "bot_return=yes\nremove_station=yes\nremoval_guard=yes\n";
    assert!(!super::super::helper_is_current(old));
    assert!(!super::super::helper_is_current(&format!(
        "{old}bot_settings_merge=no\n"
    )));
    assert!(super::super::helper_is_current(&format!(
        "{old}bot_settings_merge=yes\n"
    )));
}

/// Execute the real transfer writer against synthetic files, with service/ownership commands
/// replaced locally; this exercises station-side merging without opening an SSH connection.
fn transfer_pairing_fixture(
    name: &str,
    current: &str,
    incoming: &str,
) -> (std::process::Output, String, bool) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let dir = std::env::temp_dir().join(format!(
        "moon-remote-transfer-{name}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("telegram.json");
    std::fs::write(&path, current).unwrap();
    let input = dir.join("incoming.json");
    let stopped_path = dir.join("stopped");
    std::fs::write(&input, incoming).unwrap();
    let writer = crate::script::HELPER
        .split("cmd_put_pairing() {")
        .nth(1)
        .unwrap()
        .split("\n# ctl;")
        .next()
        .unwrap();
    let code = format!(
        r#"
set -eu
PAIRING={pairing}
UNIT=synthetic-unit
die() {{ echo "$*" >&2; exit 1; }}
systemctl() {{ test "$*" = 'stop synthetic-unit'; touch {stopped}; }}
install() {{
    test "$1 $2 $3 $4 $5 $6" = '-m 600 -o moon-station -g moon-station'
    cp "$7" "$8"
}}
cmd_put_pairing() {{{writer}
cmd_put_pairing <{incoming}
"#,
        pairing = crate::script::sh_quote(&path.to_string_lossy().replace('\\', "/")),
        incoming = crate::script::sh_quote(&input.to_string_lossy().replace('\\', "/")),
        stopped = crate::script::sh_quote(&stopped_path.to_string_lossy().replace('\\', "/")),
    );
    let mut child = Command::new("sh")
        // The MSVC build wrapper disables conversion for cmd.exe, but the fixture's Windows
        // Python must receive Windows paths for shell-created temporary files.
        .env_remove("MSYS_NO_PATHCONV")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("POSIX sh is required for transfer fixtures");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(code.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    let stopped = stopped_path.exists();
    std::fs::remove_dir_all(&dir).unwrap();
    (output, saved, stopped)
}

/// A raw transfer from an older terminal must retain layout/opaque keys while changing sent fields.
#[test]
fn transfer_keeps_station_bot_fields_absent_from_older_payloads() {
    let (ok, text, stopped) = transfer_pairing_fixture(
        "old-fields",
        r#"{"authorized_chat_ids":[7],"bot":{"report_view":"cores","message_layout":{"card":{"coin_hashtag":false}},"future_option":42}}"#,
        r#"{"authorized_chat_ids":[9],"owner_chat_id":9,"bot":{"report_view":"exchanges","another_future_key":{"on":true}}}"#,
    );
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let saved: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(stopped, "a successful transfer must stop the station");
    assert_eq!(saved["authorized_chat_ids"], serde_json::json!([9]));
    assert_eq!(saved["bot"]["report_view"], "exchanges");
    assert_eq!(
        saved["bot"]["message_layout"]["card"]["coin_hashtag"],
        false
    );
    assert_eq!(saved["bot"]["future_option"], 42);
    assert_eq!(
        saved["bot"]["another_future_key"],
        serde_json::json!({"on": true})
    );
}

/// Treating an explicit layout as absent would prevent replacement; omitting bot keeps its whole.
#[test]
fn transfer_replaces_supplied_layout_but_keeps_an_omitted_bot() {
    let current = r#"{"bot":{"message_layout":{"custom":true},"future_option":42}}"#;
    let (ok, text, _) = transfer_pairing_fixture(
        "layout-reset",
        current,
        r#"{"bot":{"message_layout":{},"future_option":null}}"#,
    );
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let saved: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(saved["bot"]["message_layout"], serde_json::json!({}));
    assert!(saved["bot"]["future_option"].is_null());
    let (ok, text, _) = transfer_pairing_fixture("no-bot", current, "{}");
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["bot"],
        serde_json::json!({"message_layout": {"custom": true}, "future_option": 42})
    );
}

/// Replacing before decoding JSON would destroy settings; stopping before incoming validation
/// would leave the station down for a malformed request.
#[test]
fn transfer_refuses_corrupt_settings_without_overwriting() {
    for (name, current, incoming, expect_stop) in [
        ("bad-current", "{ broken", "{}", true),
        (
            "bad-incoming",
            r#"{"bot":{"future_option":42}}"#,
            "{ broken",
            false,
        ),
        (
            "bad-bot",
            r#"{"bot":{"future_option":42}}"#,
            r#"{"bot":7}"#,
            false,
        ),
        ("empty", r#"{"bot":{"future_option":42}}"#, "", false),
        ("array", r#"{"bot":{"future_option":42}}"#, "[]", false),
    ] {
        let (ok, saved, stopped) = transfer_pairing_fixture(name, current, incoming);
        assert!(!ok.status.success(), "{name}");
        assert_eq!(saved, current, "{name}");
        assert_eq!(stopped, expect_stop, "{name}");
    }
}

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

/// Reusing token A after ServerToken replaced it with B deletes B and associates B's grants with A.
#[test]
fn a_retry_returns_the_current_bot_instead_of_a_superseded_snapshot() {
    let old = ReturnedBot {
        token: Secret::new("synthetic-bot-A"),
        access: Access {
            authorized_chat_ids: vec![42],
            owner_chat_id: Some(42),
            chat_access: Vec::new(),
            ..Access::default()
        },
    };
    let mut remembered = None;
    let returned = read_then_remove(
        true,
        || {
            current_or_recovered(old, true, || {
                Ok(ReturnedBot {
                    token: Secret::new("synthetic-bot-B"),
                    access: Access {
                        authorized_chat_ids: vec![73],
                        owner_chat_id: Some(73),
                        chat_access: Vec::new(),
                        ..Access::default()
                    },
                })
            })
        },
        &mut |bot| remembered = Some(bot),
        |_, _| Ok(()),
    )
    .unwrap()
    .unwrap();
    assert_eq!(returned.token.expose(), "synthetic-bot-B");
    assert_eq!(returned.access.authorized_chat_ids, [73]);
    assert_eq!(returned.access.owner_chat_id, Some(73));
    assert_eq!(remembered.unwrap().token.expose(), "synthetic-bot-B");
}

/// Requiring an already-deleted credential after partial removal would strand the only copy.
#[test]
fn a_partial_removal_retry_uses_the_retained_bot_when_credential_is_gone() {
    let old = ReturnedBot {
        token: Secret::new("synthetic-retained"),
        access: Access::default(),
    };
    let returned = current_or_recovered(old, false, || panic!("credential is gone")).unwrap();
    assert_eq!(returned.token.expose(), "synthetic-retained");
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
        ..Access::default()
    };
    let json: serde_json::Value = serde_json::to_value(&pairing).unwrap();
    assert_eq!(json["authorized_chat_ids"][0], 42);
    assert_eq!(json["owner_chat_id"], 42);
    assert_eq!(json["chat_access"][0]["core_uids"][1], 3);
}
