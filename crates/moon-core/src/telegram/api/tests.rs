//! Wire-shape regressions for Telegram's mutually exclusive reply markup kinds.

use super::{
    InlineKeyboardButton, InlineKeyboardMarkup, KeyboardButton, ReplyKeyboardMarkup, ReplyMarkup,
    SendMessageReq,
};
use serde_json::json;

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
