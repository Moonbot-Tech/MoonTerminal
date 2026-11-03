use moon_core::feed::{CoreTelegramState, TelegramCmd};

use super::{RemoteBlock, RemoteInvalid, bot_request};

/// An empty token box keeps the saved token; it must never reach the core as `Some("")`, which
/// moonproto documents as "clear the token".
///
/// Mutation: send the box's text as typed. Saving the own-bot switch with an untouched box would
/// then wipe the trader's token.
#[test]
fn an_empty_token_box_keeps_the_saved_token() {
    assert_eq!(
        bot_request(true, "  ", true),
        Ok(TelegramCmd::SetOutgoingBot {
            own_bot: true,
            token: None,
        })
    );
    assert_eq!(
        bot_request(true, " 123:abc ", false),
        Ok(TelegramCmd::SetOutgoingBot {
            own_bot: true,
            token: Some("123:abc".to_string()),
        })
    );
}

/// Switching the own bot on with no token anywhere is refused before the send.
///
/// Mutation: drop the check. The core would then route every report through a bot it cannot log in
/// as, and the trader learns it only when the channel stays silent.
#[test]
fn an_own_bot_without_any_token_is_refused() {
    assert_eq!(
        bot_request(true, "", false),
        Err(RemoteInvalid::OwnBotNeedsToken)
    );
    // Switching back to the shared bot needs no token.
    assert!(bot_request(false, "", false).is_ok());
}

/// The switch shows the trader's choice until it is saved, and the core's otherwise.
#[test]
fn the_switch_shows_the_pending_choice_over_the_core() {
    let snapshot = CoreTelegramState {
        own_bot: true,
        ..Default::default()
    };
    let mut block = RemoteBlock::default();
    assert!(block.own_bot_shown(Some(&snapshot)));
    block.own_bot = Some(false);
    assert!(!block.own_bot_shown(Some(&snapshot)));
    assert!(!block.own_bot_shown(None));
}
