//! Pins for the Control section's screens that need no live session.

use moon_core::session::CoreRunState;
use moon_core::telegram::menu_action::{ControlAction, ControlSwitch, ControlTarget, MenuAction};

use super::{confirm, state_marks, valid_coin};

fn action_of(button: &moon_core::telegram::api::InlineKeyboardButton) -> Option<MenuAction> {
    button
        .callback_data
        .as_deref()
        .and_then(MenuAction::parse_callback)
}

/// A dangerous command must not run on the first press: its confirmation sends the SAME action
/// confirmed, and the way back leads to its card, not to the command.
#[test]
fn a_confirmation_sends_the_same_action_confirmed() {
    let _locale = crate::test_locale::force("en");
    let ask = ControlAction::Run {
        target: ControlTarget::All,
        switch: ControlSwitch::Trading,
        on: false,
        confirmed: false,
    };
    let (_, lines, rows) = confirm("stop?".into(), ask, ControlAction::All);
    assert_eq!(lines, vec!["stop?".to_string()]);
    assert_eq!(
        action_of(&rows[0][0]),
        Some(MenuAction::Control(ControlAction::Run {
            target: ControlTarget::All,
            switch: ControlSwitch::Trading,
            on: false,
            confirmed: true,
        }))
    );
    assert_eq!(
        action_of(&rows[0][1]),
        Some(MenuAction::Control(ControlAction::All))
    );
    let (_, _, rows) = confirm(
        "cancel?".into(),
        ControlAction::CancelAll {
            core: 7,
            confirmed: false,
        },
        ControlAction::Core(7),
    );
    assert_eq!(
        action_of(&rows[0][0]),
        Some(MenuAction::Control(ControlAction::CancelAll {
            core: 7,
            confirmed: true
        }))
    );
}

/// The list shows link and trading at a glance; an unknown trading state is not drawn as paused.
#[test]
fn state_marks_tell_link_and_trading() {
    let mut state = CoreRunState {
        online: true,
        trading: Some(true),
        ..CoreRunState::default()
    };
    assert_eq!(state_marks(&state), "\u{1f7e2}\u{25b6}\u{fe0f}");
    state.online = false;
    state.trading = None;
    assert_eq!(state_marks(&state), "\u{1f534}\u{2754}");
}

/// The answer to "which coin" is written into the core's list as one token: a comma or a space
/// would smuggle several coins (or a broken entry) in at once.
#[test]
fn a_coin_answer_is_one_token() {
    assert!(valid_coin("ADA"));
    assert!(valid_coin("1kBONKPERP"));
    assert!(valid_coin("BTC_RP"));
    assert!(!valid_coin(""));
    assert!(!valid_coin("ADA,BTC"));
    assert!(!valid_coin("ADA BTC"));
    assert!(!valid_coin(&"A".repeat(31)));
}
