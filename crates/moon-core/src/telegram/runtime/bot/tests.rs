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

/// The command list follows the host's labels in menu order and leaves out a command without a
/// description, or with a blank one Telegram would refuse the whole list for.
#[test]
fn the_command_list_follows_the_labels() {
    let labels = std::collections::BTreeMap::from([
        ("command_help".to_owned(), "Help".to_owned()),
        ("command_today".to_owned(), "Today".to_owned()),
        ("command_unknown".to_owned(), "x".to_owned()),
        ("command_month".to_owned(), "  ".to_owned()),
    ]);
    assert_eq!(
        super::bot_commands(&labels),
        vec![
            ("today".to_owned(), "Today".to_owned()),
            ("help".to_owned(), "Help".to_owned()),
        ]
    );
}
