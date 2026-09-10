//! Navigation regressions: localized reply text must not bypass slash-command addressing.

use super::{ParsedCommand, parse_reply_button, parse_text};
use std::collections::BTreeMap;

/// A mismatched bot suffix or trailing argument must never become a valid navigation action.
#[test]
fn navigation_commands_keep_address_and_argument_guards() {
    assert_eq!(
        parse_text("/start@MyBot", Some("mybot")),
        ParsedCommand::Start
    );
    assert_eq!(
        parse_text("/help@MyBot", Some("mybot")),
        ParsedCommand::Help
    );
    for text in ["/start@other", "/help@other"] {
        assert_eq!(parse_text(text, Some("mybot")), ParsedCommand::Unknown);
    }
    for text in ["/start extra", "/help extra"] {
        assert_eq!(
            parse_text(text, Some("mybot")),
            ParsedCommand::InvalidArgument
        );
    }
}

/// Exact old-locale labels remain usable; partial text and command-shaped labels stay inert.
#[test]
fn reply_labels_accept_all_locales_without_loose_command_matching() {
    let labels = BTreeMap::from([
        ("button_miniapp_ru".into(), "Открыть Mini App".into()),
        ("button_miniapp_en".into(), "Open Mini App".into()),
        ("button_help_es".into(), "Ayuda".into()),
        ("button_help_ru".into(), "/help@other".into()),
    ]);
    for text in ["Открыть Mini App", "Open Mini App"] {
        assert_eq!(parse_reply_button(text, &labels), ParsedCommand::MiniApp);
    }
    assert_eq!(parse_reply_button("Ayuda", &labels), ParsedCommand::Help);
    for text in ["Open Mini", "/help@other", "", "Open Mini App extra"] {
        assert_eq!(parse_reply_button(text, &labels), ParsedCommand::Unknown);
    }
}
