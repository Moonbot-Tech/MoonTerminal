//! Wire-shape regressions for Telegram's mutually exclusive reply markup kinds.

use super::{
    InlineKeyboardButton, InlineKeyboardMarkup, KeyboardButton, ReplyKeyboardMarkup, ReplyMarkup,
    text_message_request,
};
use serde_json::json;

/// Telegram forbids editing messages carrying ReplyKeyboardRemove: reports must start inline.
#[test]
fn rich_reports_are_sent_with_editable_inline_navigation() {
    let keyboard = ReplyMarkup::Inline(InlineKeyboardMarkup::from_rows(vec![vec![
        InlineKeyboardButton::callback("Refresh", "r:e:t:0"),
    ]]));
    let (method, body) = super::rich_request(7, None, "<p>Report</p>", &keyboard);
    assert_eq!(method, "sendRichMessage");
    assert_eq!(
        body["reply_markup"],
        json!({"inline_keyboard":[[{"text":"Refresh","callback_data":"r:e:t:0"}]]})
    );
    assert!(body.get("message_id").is_none());
    assert!(body["reply_markup"].get("remove_keyboard").is_none());
    let (method, edited) = super::rich_request(7, Some(50), "<p>Updated</p>", &keyboard);
    assert_eq!(method, "editMessageText");
    assert_eq!(edited["message_id"], 50);
    assert_eq!(edited["chat_id"], 7);
    assert_eq!(edited["reply_markup"], body["reply_markup"]);
}

/// Only an unchanged edit is a harmless no-op; rate limits and other methods remain failures.
#[test]
fn unchanged_edits_do_not_report_transport_failure() {
    let error = super::ApiError::Telegram {
        description: "Bad Request: message is not modified".into(),
        retry_after_secs: None,
    };
    assert!(super::is_unchanged_edit("editMessageText", &error));
    assert!(super::is_unchanged_edit("editMessageReplyMarkup", &error));
    assert!(!super::is_unchanged_edit("sendRichMessage", &error));
    let limited = super::ApiError::Telegram {
        description: "message is not modified".into(),
        retry_after_secs: Some(30),
    };
    assert!(!super::is_unchanged_edit("editMessageText", &limited));
}

/// Menu writes must include a private chat scope and Telegram's tagged web_app wire shape.
#[test]
fn native_menu_request_is_chat_scoped_and_clears_without_a_stale_url() {
    let button = super::MenuButton::WebApp {
        text: "Open".into(),
        web_app: super::WebAppInfo {
            url: "https://example.invalid".into(),
        },
    };
    assert_eq!(
        serde_json::to_value(super::SetChatMenuButtonReq {
            chat_id: 7,
            menu_button: &button,
        })
        .unwrap(),
        json!({"chat_id":7,"menu_button":{"type":"web_app","text":"Open","web_app":{"url":"https://example.invalid"}}})
    );
    assert_eq!(
        serde_json::to_value(super::SetChatMenuButtonReq {
            chat_id: 7,
            menu_button: &super::MenuButton::Commands,
        })
        .unwrap(),
        json!({"chat_id":7,"menu_button":{"type":"commands"}})
    );
}

/// A Rust enum wrapper or an inline-shaped persistent menu would be rejected by Telegram.
#[test]
fn send_message_serializes_persistent_text_keyboard_without_enum_wrapper() {
    let markup = ReplyMarkup::Reply(ReplyKeyboardMarkup {
        keyboard: vec![vec![
            KeyboardButton {
                text: "Open Mini App".into(),
                style: Some("primary".into()),
            },
            KeyboardButton {
                text: "Help".into(),
                style: None,
            },
        ]],
        resize_keyboard: true,
        is_persistent: true,
    });
    let request = text_message_request(7, "Welcome", Some(&markup));
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        json!({
            "chat_id": 7,
            "text": "Welcome",
            "reply_markup": {
                "keyboard": [[{"text": "Open Mini App", "style": "primary"}, {"text": "Help"}]],
                "resize_keyboard": true,
                "is_persistent": true
            }
        })
    );
}

/// Adding reply keyboards must preserve the inline web_app contract used for signed launches.
#[test]
fn inline_launcher_keeps_web_app_wire_shape() {
    let markup = ReplyMarkup::Inline(InlineKeyboardMarkup::from_rows(vec![vec![
        InlineKeyboardButton::web_app("Open", "https://example.invalid"),
    ]]));
    let request = text_message_request(7, "Ready", Some(&markup));
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        json!({
            "chat_id": 7,
            "text": "Ready",
            "reply_markup": {"inline_keyboard": [[{
                "text": "Open", "web_app": {"url": "https://example.invalid"}
            }]]}
        })
    );
}

/// Cleanup no-ops are quiet, but authentication and rate limits remain observable failures.
#[test]
fn unavailable_deletion_is_scoped_and_does_not_hide_rate_limits() {
    let missing = super::ApiError::Telegram {
        description: "Bad Request: message to delete not found".into(),
        retry_after_secs: None,
    };
    assert!(super::is_unavailable_delete("deleteMessage", &missing));
    assert!(!super::is_unavailable_delete("sendMessage", &missing));
    let expired = super::ApiError::Telegram {
        description: "Bad Request: message can't be deleted".into(),
        retry_after_secs: None,
    };
    assert!(super::is_unavailable_delete("deleteMessage", &expired));
    let limited = super::ApiError::Telegram {
        description: "Bad Request: message can't be deleted".into(),
        retry_after_secs: Some(60),
    };
    assert!(!super::is_unavailable_delete("deleteMessage", &limited));
    let unauthorized = super::ApiError::Telegram {
        description: "Unauthorized".into(),
        retry_after_secs: None,
    };
    assert!(!super::is_unavailable_delete(
        "deleteMessage",
        &unauthorized
    ));
}

/// Two pollers on one token answer each other with 409 forever; the status must name that rather
/// than a transport outage, and the error must not be retried in place.
#[test]
fn a_409_is_a_conflict_and_is_not_retried() {
    let conflict = super::refusal(
        409,
        "Conflict: terminated by other getUpdates request; make sure that only one bot instance \
         is running"
            .into(),
        None,
    );
    assert_eq!(conflict, super::ApiError::Conflict);
    assert!(!super::is_retryable(&conflict));
    let webhook = super::refusal(
        409,
        "Conflict: can't use getUpdates method while webhook is active".into(),
        None,
    );
    assert_eq!(webhook, super::ApiError::Conflict);
    assert!(matches!(
        super::refusal(400, "Bad Request".into(), None),
        super::ApiError::Telegram { .. }
    ));
}

/// Only a refusal about one chat is that chat's state; a bad token, a rate limit or a transport
/// failure stays the bot's health.
#[test]
fn only_chat_refusals_count_as_an_unreachable_chat() {
    use super::ApiError;
    let telegram = |description: &str, retry_after_secs| ApiError::Telegram {
        description: description.into(),
        retry_after_secs,
    };
    for chat in [
        "Forbidden: bot was blocked by the user",
        "Forbidden: user is deactivated",
        "Bad Request: user not found",
        "Bad Request: chat not found",
    ] {
        assert!(super::is_unreachable_chat(&telegram(chat, None)), "{chat}");
    }
    for bot in [
        telegram("Unauthorized", None),
        telegram("Too Many Requests: retry after 5", Some(5)),
        telegram("Bad Request: message text is empty", None),
        ApiError::Transport,
        ApiError::Timeout,
        ApiError::Conflict,
    ] {
        assert!(!super::is_unreachable_chat(&bot), "{bot}");
    }
}

/// A notification must travel as HTML with previews off. A plain sendMessage that gained either
/// field would change every existing command reply.
#[test]
fn send_html_sets_parse_mode_and_disables_previews() {
    let html = super::send_html_request(7, "<b>filled</b>");
    let body = serde_json::to_value(&html).unwrap();
    assert_eq!(body["parse_mode"], "HTML");
    assert_eq!(body["link_preview_options"], json!({"is_disabled": true}));
    assert_eq!(body["text"], "<b>filled</b>");
    assert_eq!(body["chat_id"], 7);

    let plain = text_message_request(7, "filled", None);
    let plain_body = serde_json::to_value(&plain).unwrap();
    assert!(plain_body.get("parse_mode").is_none(), "{plain_body}");
    assert!(
        plain_body.get("link_preview_options").is_none(),
        "{plain_body}"
    );
    assert_eq!(plain_body, json!({"chat_id": 7, "text": "filled"}));
}

/// Bad markup, an over-long body and empty text must leave the queue. A 429, a 5xx and a timeout
/// must stay, or one bad minute would drop a notification.
#[test]
fn only_permanent_payload_descriptions_are_dropped() {
    use super::ApiError;
    let telegram = |description: &str, retry_after_secs| ApiError::Telegram {
        description: description.into(),
        retry_after_secs,
    };
    for permanent in [
        "Bad Request: can't parse entities",
        "Bad Request: can't parse entities: Unsupported start tag \"x\" at byte offset 1",
        "Bad Request: message is too long",
        "Bad Request: text must be non-empty",
        "Bad Request: message text is empty",
    ] {
        assert!(
            super::is_permanent_payload_error(&telegram(permanent, None)),
            "{permanent}"
        );
    }
    for retryable in [
        telegram("Too Many Requests: retry after 5", Some(5)),
        telegram("Internal Server Error", None),
        telegram("Bad Gateway", None),
        telegram("Bad Request: message is too long", Some(3)),
        ApiError::Timeout,
        ApiError::Transport,
        ApiError::Conflict,
    ] {
        assert!(
            !super::is_permanent_payload_error(&retryable),
            "{retryable}"
        );
    }
}
