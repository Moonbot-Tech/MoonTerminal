use super::*;

#[test]
fn reads_an_unpaired_bot_and_its_code() {
    let text = "telegram: bot starting, 0 chat(s) paired, Mini App on, zone UTC\n\
                telegram: no chat is paired — send /pair 482913 to the bot within 10 minutes\n\
                telegram: bot Unpaired, Mini App Tunneling { port: 33137, url: \"https://x.trycloudflare.com\" }\n";
    let state = BotState::parse(text);
    assert!(!state.stopped && !state.paired());
    assert_eq!(state.pairing_code.as_deref(), Some("482913"));
    assert!(state.status.unwrap().starts_with("Unpaired"));
}

#[test]
fn a_paired_bot_offers_no_code_any_more() {
    let text = "telegram: no chat is paired — send /pair 482913 to the bot within 10 minutes\n\
                telegram: bot Paired { chat_count: 1 }, Mini App Tunneling { port: 1, url: \"u\" }\n";
    let state = BotState::parse(text);
    assert!(state.paired());
    assert_eq!(state.pairing_code, None);
}

#[test]
fn a_stopped_station_says_so() {
    let state = BotState::parse("tg=stopped\n");
    assert!(state.stopped && !state.paired() && state.status.is_none());
}

#[test]
fn the_pairing_file_is_what_the_station_reads() {
    let pairing = Pairing {
        authorized_chat_ids: vec![42],
        owner_chat_id: Some(42),
        chat_access: vec![TelegramChatAccess {
            chat_id: 42,
            name: "me".into(),
            core_uids: vec![1, 3],
        }],
    };
    let json: serde_json::Value = serde_json::to_value(&pairing).unwrap();
    assert_eq!(json["authorized_chat_ids"][0], 42);
    assert_eq!(json["owner_chat_id"], 42);
    assert_eq!(json["chat_access"][0]["core_uids"][1], 3);
}

#[test]
fn an_unpaired_bot_polls_and_a_starting_one_does_not() {
    let unpaired = BotState::parse("telegram: bot Unpaired, Mini App Stopped\n");
    assert!(unpaired.polling() && !unpaired.paired());
    let starting =
        BotState::parse("telegram: bot starting, 0 chat(s) paired, Mini App on, zone UTC\n");
    assert!(!starting.polling());
    let conflict = BotState::parse("telegram: bot Conflict, Mini App Stopped\n");
    assert!(!conflict.polling());
}
