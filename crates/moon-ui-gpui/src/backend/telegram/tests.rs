//! Cleanup identity regressions use empty credentials, so no transport or UI is launched.

use super::TelegramState;
use moon_core::config::{Secret, TelegramConfig};

/// Every visible emoji button must resolve through exact localized aliases without guessing text.
#[test]
fn persistent_navigation_buttons_have_recognized_commands() {
    let labels = super::telegram_labels();
    let moon_core::telegram::api::ReplyMarkup::Reply(markup) = super::navigation_keyboard() else {
        panic!("expected persistent keyboard")
    };
    assert!(markup.is_persistent);
    for row in markup.keyboard {
        for button in row {
            assert_ne!(
                moon_core::telegram::commands::parse_reply_button(&button.text, &labels),
                moon_core::telegram::commands::ParsedCommand::Unknown,
                "{}",
                button.text
            );
        }
    }
}

/// A detached read must retain exclusive admission even when its bot service is replaced.
#[test]
fn pending_report_survives_service_replacement() {
    let config = TelegramConfig::default();
    let mut state = TelegramState::new(&config);
    state.report_pending = true;
    state.start_saved(&config);
    assert!(state.report_pending);
}

/// The same restart helper used after pairing reset must retain retired menu recipients.
#[test]
fn revoked_menu_identities_survive_service_replacement() {
    let before = TelegramConfig {
        authorized_chat_ids: vec![7, 8],
        ..TelegramConfig::default()
    };
    let saved = TelegramConfig {
        authorized_chat_ids: vec![8],
        ..TelegramConfig::default()
    };
    let mut state = TelegramState::new(&before);
    state.remember_menu_cleanup(&before, &saved);
    state.restart();
    state.start_saved(&saved);
    assert_eq!(state.retired_menu_chats, vec![7]);
    assert!(
        state.service.is_none(),
        "empty test credentials must not start transport"
    );
    state.start_saved(&saved);
    assert_eq!(
        state.retired_menu_chats,
        vec![7],
        "a second retirement must not lose pending cleanup"
    );
}

/// A new bot must never receive cleanup requests for the previous bot's users.
#[test]
fn token_change_discards_old_bot_menu_identities() {
    let before = TelegramConfig {
        authorized_chat_ids: vec![7],
        ..TelegramConfig::default()
    };
    let saved = TelegramConfig::default();
    let mut state = TelegramState::new(&before);
    state.remember_menu_cleanup(&before, &saved);
    let new_bot = TelegramConfig {
        token: Secret::new("fixture-only"),
        ..saved.clone()
    };
    // Only compare credential identities; never start the non-empty fixture credential.
    state.remember_menu_cleanup(&saved, &new_bot);
    assert!(state.retired_menu_chats.is_empty());
}
