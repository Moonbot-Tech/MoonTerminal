//! Restored bot ownership and Settings draft preservation.
use super::{can_restore_bot, restore_bot, save_returned_bot};
use moon_core::config::AppConfig;
use moon_core::config::telegram_access::TelegramChatAccess;
use moon_core::config::{Secret, TelegramConfig};
use moon_core::station_api::Access;
use moon_remote::station::bot::ReturnedBot;

/// Overwriting either a saved or unsaved local token discards the user's second bot.
#[test]
fn local_saved_or_draft_bots_block_restoration() {
    let empty = TelegramConfig::default();
    let own = TelegramConfig {
        token: Secret::new("second-synthetic-bot"),
        ..TelegramConfig::default()
    };
    assert!(can_restore_bot(&empty, None));
    assert!(can_restore_bot(&empty, Some(&empty)));
    assert!(!can_restore_bot(&own, None));
    assert!(!can_restore_bot(&own, Some(&empty)));
    assert!(!can_restore_bot(&empty, Some(&own)));
}

/// Omitting Access loses viewer grants; replacing switches loses the terminal preference.
#[test]
fn restored_configs_keep_mini_app_and_all_access() {
    let returned = ReturnedBot {
        token: Secret::new("station-synthetic-bot"),
        access: Access {
            authorized_chat_ids: vec![42, 73],
            owner_chat_id: Some(42),
            chat_access: vec![TelegramChatAccess {
                chat_id: 73,
                name: "Observer".into(),
                core_uids: vec![9, 12],
            }],
        },
    };
    let mut saved = TelegramConfig::default();
    let mut draft = TelegramConfig {
        mini_app_enabled: true,
        ..TelegramConfig::default()
    };
    restore_bot(&mut saved, &returned);
    restore_bot(&mut draft, &returned);
    for config in [&saved, &draft] {
        assert_eq!(config.token.expose(), "station-synthetic-bot");
        assert_eq!(config.authorized_chat_ids, [42, 73]);
        assert_eq!(config.owner_chat_id, Some(42));
        assert_eq!(config.chat_access[0].chat_id, 73);
        assert_eq!(config.chat_access[0].name, "Observer");
        assert_eq!(config.chat_access[0].core_uids, [9, 12]);
    }
    assert!(!saved.mini_app_enabled);
    assert!(draft.mini_app_enabled);
}

/// Publishing before disk commit would start an unsaved bot and corrupt an open Settings draft.
#[test]
fn failed_save_leaves_saved_and_draft_bot_fields_unchanged() {
    let mut saved = AppConfig::headless(Vec::new());
    let mut draft = Some(saved.clone());
    draft.as_mut().unwrap().telegram.mini_app_enabled = true;
    let returned = ReturnedBot {
        token: Secret::new("synthetic-restored"),
        access: Access::default(),
    };
    let result = save_returned_bot(&mut saved, &mut draft, &returned, |candidate| {
        assert_eq!(candidate.telegram.token.expose(), "synthetic-restored");
        Err(anyhow::anyhow!("synthetic disk failure"))
    });
    assert!(result.is_err());
    assert!(saved.telegram.token.is_empty());
    assert!(draft.as_ref().unwrap().telegram.token.is_empty());
    assert!(draft.as_ref().unwrap().telegram.mini_app_enabled);
}

/// Skipping the draft update after saving lets a later Settings Save erase the restored bot.
#[test]
fn committed_return_updates_both_configs_after_save() {
    let mut saved = AppConfig::headless(Vec::new());
    let mut draft = Some(saved.clone());
    draft.as_mut().unwrap().telegram.mini_app_enabled = true;
    let returned = ReturnedBot {
        token: Secret::new("synthetic-restored"),
        access: Access {
            authorized_chat_ids: vec![42],
            owner_chat_id: Some(42),
            chat_access: Vec::new(),
        },
    };
    let committed = std::cell::Cell::new(false);
    assert!(
        save_returned_bot(&mut saved, &mut draft, &returned, |candidate| {
            assert_eq!(candidate.telegram.authorized_chat_ids, [42]);
            committed.set(true);
            Ok(())
        })
        .unwrap()
    );
    assert!(committed.get());
    for config in [&saved, draft.as_ref().unwrap()] {
        assert_eq!(config.telegram.token.expose(), "synthetic-restored");
        assert_eq!(config.telegram.owner_chat_id, Some(42));
    }
    assert!(!saved.telegram.mini_app_enabled);
    assert!(draft.as_ref().unwrap().telegram.mini_app_enabled);
}

/// Rechecking only the job-start snapshot overwrites a local bot saved while SSH was running.
#[test]
fn local_bot_created_during_ssh_is_never_saved_over() {
    let mut saved = AppConfig::headless(Vec::new());
    let mut draft = Some(saved.clone());
    draft.as_mut().unwrap().telegram.token = Secret::new("new-local-bot");
    let returned = ReturnedBot {
        token: Secret::new("synthetic-station"),
        access: Access::default(),
    };
    assert!(
        !save_returned_bot(&mut saved, &mut draft, &returned, |_| panic!(
            "must not save over a local bot"
        ))
        .unwrap()
    );
    assert!(saved.telegram.token.is_empty());
    assert_eq!(
        draft.as_ref().unwrap().telegram.token.expose(),
        "new-local-bot"
    );
}

/// Reintroducing the local-only station_begin shortcut after a failed save can start a second
/// poller: server-token actions remain available between attempts. This binary-only owner cannot
/// be constructed without the production host, so check its dispatch boundary as source text.
#[test]
fn bot_off_retries_never_publish_locally_before_dispatching_server_work() {
    let source = include_str!("../station.rs");
    let begin = source.split("    fn station_begin(").nth(1).unwrap();
    let begin = begin.split("    fn station_next(").next().unwrap();
    assert!(!begin.contains("self.station_apply_returned()"));
    assert!(!begin.contains("self.telegram.resume("));
}
