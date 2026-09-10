//! Navigation regressions: localized reply text must not bypass slash-command addressing.

use super::{ParsedCommand, parse_reply_button, parse_text};
use std::collections::BTreeMap;

/// A callback cannot borrow authorization from a different private chat or a group message.
#[test]
fn report_callbacks_require_matching_private_sender_identity() {
    let mut update: crate::telegram::api::Update = serde_json::from_value(serde_json::json!({
        "update_id":1,"callback_query":{"id":"fixture","from":{"id":7},
        "message":{"message_id":10,"chat":{"id":7,"type":"private"}},"data":"r:c:t:0"}
    }))
    .unwrap();
    assert!(matches!(
        super::parse_update(&update, None).unwrap().command,
        ParsedCommand::Report(_)
    ));
    update.callback_query.as_mut().unwrap().from.id = 8;
    assert!(super::parse_update(&update, None).is_none());
    update.callback_query.as_mut().unwrap().from.id = 7;
    update
        .callback_query
        .as_mut()
        .unwrap()
        .message
        .as_mut()
        .unwrap()
        .chat
        .kind = "group".into();
    assert!(super::parse_update(&update, None).is_none());
}

/// Reports retain the bot suffix guard and reject oversized or reversed custom ranges.
#[test]
fn report_commands_preserve_address_and_range_guards() {
    assert_eq!(
        parse_text("/report@other 2026-09-01 2026-09-10", Some("mine")),
        ParsedCommand::Unknown
    );
    for text in [
        "/report 2026-09-10 2026-09-01",
        "/daily 2020-01-01 2026-09-10",
        "/report 2026-09-01",
    ] {
        assert_eq!(parse_text(text, None), ParsedCommand::InvalidArgument);
    }
    assert!(
        matches!(parse_text("/daily@mine 2026-09-01 2026-09-10",Some("mine")),ParsedCommand::Report(request) if request.daily)
    );
}

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
