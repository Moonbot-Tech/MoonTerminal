//! Wire-shape regressions for Telegram's mutually exclusive reply markup kinds.

use super::{
    InlineKeyboardButton, InlineKeyboardMarkup, KeyboardButton, ReplyKeyboardMarkup, ReplyMarkup,
    SendMessageReq,
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
    let request = SendMessageReq {
        chat_id: 7,
        text: "Welcome",
        reply_markup: Some(&markup),
    };
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
    let request = SendMessageReq {
        chat_id: 7,
        text: "Ready",
        reply_markup: Some(&markup),
    };
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
