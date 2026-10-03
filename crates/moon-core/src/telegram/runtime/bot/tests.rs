//! Chat tidying decisions; no Bot API requests.

/// An update carrying a private message sent `age` seconds ago.
fn message(age: i64) -> crate::telegram::api::Update {
    let now = crate::util::time::now_unix_ms_i64() / 1000;
    serde_json::from_value(serde_json::json!({
        "update_id": 1,
        "message": {"message_id": 42, "date": now - age, "chat": {"id": 7, "type": "private"},
            "text": "Today"}
    }))
    .unwrap()
}

/// A reply-keyboard press is removed once answered; typed text, a press too old for Telegram to
/// delete, and a callback are left alone.
#[test]
fn only_a_fresh_button_press_is_removed() {
    assert_eq!(super::pressed_button(&message(5), true), Some(42));
    assert_eq!(super::pressed_button(&message(5), false), None);
    assert_eq!(super::pressed_button(&message(49 * 60 * 60), true), None);
    let callback: crate::telegram::api::Update = serde_json::from_value(serde_json::json!({
        "update_id": 2,
        "callback_query": {"id": "c", "from": {"id": 7},
            "message": {"message_id": 10, "chat": {"id": 7, "type": "private"}}, "data": "m:r"}
    }))
    .unwrap();
    assert_eq!(super::pressed_button(&callback, true), None);
}
