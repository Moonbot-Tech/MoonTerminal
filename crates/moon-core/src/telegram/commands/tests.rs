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

/// The station's "Update" button is its own callback, held to the same private-sender identity as
/// a report's; any other data stays unknown rather than falling into an update.
#[test]
fn the_station_update_callback_is_exact_and_private() {
    let mut update: crate::telegram::api::Update = serde_json::from_value(serde_json::json!({
        "update_id":1,"callback_query":{"id":"fixture","from":{"id":7},
        "message":{"message_id":10,"chat":{"id":7,"type":"private"}},"data":"station:update"}
    }))
    .unwrap();
    assert_eq!(
        super::parse_update(&update, None).unwrap().command,
        ParsedCommand::StationUpdate
    );
    update.callback_query.as_mut().unwrap().data = Some("station:update:v9".into());
    assert_eq!(
        super::parse_update(&update, None).unwrap().command,
        ParsedCommand::Unknown
    );
    update.callback_query.as_mut().unwrap().data = Some("station:update".into());
    update.callback_query.as_mut().unwrap().from.id = 8;
    assert!(super::parse_update(&update, None).is_none());
}

/// `/status` and the station's button reach the status; an argument or another bot's suffix
/// does not.
#[test]
fn station_status_keeps_address_and_argument_guards() {
    assert_eq!(parse_text("/status", None), ParsedCommand::StationStatus);
    assert_eq!(
        parse_text("/status@MyBot", Some("mybot")),
        ParsedCommand::StationStatus
    );
    assert_eq!(
        parse_text("/status@other", Some("mybot")),
        ParsedCommand::Unknown
    );
    assert_eq!(
        parse_text("/status now", None),
        ParsedCommand::InvalidArgument
    );
    let labels = BTreeMap::from([("button_status_emoji_ru".into(), "📡 Статус".into())]);
    assert_eq!(
        parse_reply_button("📡 Статус", &labels),
        ParsedCommand::StationStatus
    );
    assert_eq!(
        parse_reply_button("📡 Статус", &BTreeMap::new()),
        ParsedCommand::Unknown
    );
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

/// Exact old-locale labels remain usable, including Ukrainian, Turkish,
/// Portuguese, and Vietnamese; partial text and command-shaped labels stay
/// inert. A loop that still skipped one of those codes would leave its reply
/// button unmatched.
#[test]
fn reply_labels_accept_all_locales_without_loose_command_matching() {
    let labels = BTreeMap::from([
        ("button_miniapp_ru".into(), "Открыть Mini App".into()),
        ("button_miniapp_en".into(), "Open Mini App".into()),
        ("button_miniapp_uk".into(), "Open Mini App UK".into()),
        ("button_miniapp_tr".into(), "Open Mini App TR".into()),
        ("button_miniapp_pt".into(), "Open Mini App PT".into()),
        ("button_miniapp_vi".into(), "Open Mini App VI".into()),
        ("button_help_es".into(), "Ayuda".into()),
        ("button_help_ru".into(), "/help@other".into()),
    ]);
    for text in [
        "Открыть Mini App",
        "Open Mini App",
        "Open Mini App UK",
        "Open Mini App TR",
        "Open Mini App PT",
        "Open Mini App VI",
    ] {
        assert_eq!(parse_reply_button(text, &labels), ParsedCommand::MiniApp);
    }
    assert_eq!(parse_reply_button("Ayuda", &labels), ParsedCommand::Help);
    for text in ["Open Mini", "/help@other", "", "Open Mini App extra"] {
        assert_eq!(parse_reply_button(text, &labels), ParsedCommand::Unknown);
    }
}

/// Section callbacks parse as menu actions under the same private-sender identity; a report
/// callback stays a report.
#[test]
fn menu_callbacks_are_their_own_namespace() {
    let mut update: crate::telegram::api::Update = serde_json::from_value(serde_json::json!({
        "update_id":1,"callback_query":{"id":"fixture","from":{"id":7},
        "message":{"message_id":10,"chat":{"id":7,"type":"private"}},"data":"m:r"}
    }))
    .unwrap();
    assert_eq!(
        super::parse_update(&update, None).unwrap().command,
        ParsedCommand::Menu(crate::telegram::menu_action::MenuAction::Report)
    );
    update.callback_query.as_mut().unwrap().data = Some("m:nope".into());
    assert_eq!(
        super::parse_update(&update, None).unwrap().command,
        ParsedCommand::Unknown
    );
    update.callback_query.as_mut().unwrap().data = Some("m:r".into());
    update.callback_query.as_mut().unwrap().from.id = 8;
    assert!(super::parse_update(&update, None).is_none());
}

/// A period command leaves the view to the bot; `/daily` names its own.
#[test]
fn period_commands_follow_the_bot_view_and_daily_names_its_own() {
    for text in [
        "/today",
        "/yesterday",
        "/month",
        "/lastmonth",
        "/hour",
        "/report 2026-09-01 2026-09-10",
    ] {
        let ParsedCommand::Report(request) = parse_text(text, None) else {
            panic!("{text}")
        };
        assert!(request.follow_view, "{text}");
    }
    for text in ["/daily", "/daily 2026-09-01 2026-09-10"] {
        let ParsedCommand::Report(request) = parse_text(text, None) else {
            panic!("{text}")
        };
        assert!(request.daily && !request.follow_view, "{text}");
    }
}

/// Every menu item's button resolves by its id in any locale; the old Overview button still opens
/// today's report.
#[test]
fn buttons_resolve_by_item_id() {
    use crate::config::telegram_menu::MenuItem;
    let mut labels = BTreeMap::new();
    for item in MenuItem::ALL {
        labels.insert(
            format!("button_{}_emoji_es", item.id()),
            format!("* {}", item.id()),
        );
    }
    labels.insert("button_home_legacy_emoji_en".into(), "📊 Overview".into());
    for item in MenuItem::ALL {
        assert_eq!(
            parse_reply_button(&format!("* {}", item.id()), &labels),
            super::button_command(item),
            "{}",
            item.id()
        );
    }
    assert_eq!(
        parse_reply_button("📊 Overview", &labels),
        ParsedCommand::Start
    );
}

/// `/settings` opens the section; its station status is a Settings action, leading back to it.
#[test]
fn the_settings_entry_points_parse() {
    use crate::telegram::menu_action::{MenuAction, SettingsAction};
    assert_eq!(
        parse_text("/settings", None),
        ParsedCommand::Menu(MenuAction::Settings(SettingsAction::Root))
    );
    assert_eq!(
        parse_text("/settings now", None),
        ParsedCommand::InvalidArgument
    );
    let data = MenuAction::Settings(SettingsAction::StationStatus).callback();
    assert_eq!(data, "m:s:st");
    assert_eq!(
        MenuAction::parse_callback(&data),
        Some(MenuAction::Settings(SettingsAction::StationStatus))
    );
}

/// The Settings status button of an earlier build still answers, as the section's status.
#[test]
fn an_earlier_settings_status_button_still_answers() {
    use crate::telegram::menu_action::{MenuAction, SettingsAction};
    let update: crate::telegram::api::Update = serde_json::from_value(serde_json::json!({
        "update_id":1,"callback_query":{"id":"c","from":{"id":7},
        "message":{"message_id":10,"chat":{"id":7,"type":"private"}},"data":"station:status"}
    }))
    .unwrap();
    assert_eq!(
        super::parse_update(&update, None).unwrap().command,
        ParsedCommand::Menu(MenuAction::Settings(SettingsAction::StationStatus))
    );
}
