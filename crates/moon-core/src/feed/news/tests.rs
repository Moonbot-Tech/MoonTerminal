use super::*;

#[test]
fn parse_frame_reads_meta_body_coins_and_tags() {
    let item = parse_frame(
        r#"{
            "meta": {
                "id": "n-1", "timeMs": 1700000000000, "recvTime": 1700000000200,
                "sendTime": 1700000000300, "source": "toa", "author": "ToA",
                "coinsAuto": ["BTC", "ETH"], "coinsSelf": ["ETH", "SOL"], "isOriginal": true
            },
            "news": { "en": ["BTC moves", "higher"] },
            "tags": { "entity": [ {"text": "ETF"}, {"text": "Macro"} ] }
        }"#,
    )
    .expect("valid frame");

    assert_eq!(item.id, "n-1");
    assert_eq!(item.time_ms, 1700000000000);
    assert_eq!(item.recv_time_ms, Some(1700000000200));
    assert_eq!(item.send_time_ms, Some(1700000000300));
    assert_eq!(item.source, "toa");
    assert_eq!(item.author.as_deref(), Some("ToA"));
    // coinsAuto then coinsSelf, de-duplicated preserving first-seen order.
    assert_eq!(item.coins, vec!["BTC", "ETH", "SOL"]);
    // Fragments joined with a single space.
    assert_eq!(item.en, "BTC moves higher");
    assert_eq!(item.tags, vec!["ETF", "Macro"]);
    assert!(item.is_original);
}

#[test]
fn numeric_id_and_seconds_time_are_accepted() {
    let item = parse_frame(r#"{"meta":{"id":42,"time":1700000000},"news":{"en":["x"]}}"#)
        .expect("numeric id, seconds time");
    assert_eq!(item.id, "42");
    // `time` (seconds) promoted to ms.
    assert_eq!(item.time_ms, 1700000000000);
}

#[test]
fn missing_id_or_meta_is_invalid() {
    assert!(parse_frame(r#"{"meta":{"timeMs":1},"news":{"en":["x"]}}"#).is_none());
    assert!(parse_frame(r#"{"meta":{"id":""}}"#).is_none());
    assert!(parse_frame(r#"{"news":{"en":["x"]}}"#).is_none());
    assert!(parse_frame("not json").is_none());
}

#[test]
fn newline_fragments_are_normalized_to_spaces() {
    let item = parse_frame(r#"{"meta":{"id":"a"},"news":{"en":["line one\r\nline two","\n three "]}}"#)
        .unwrap();
    assert_eq!(item.en, "line one line two three");
}

#[test]
fn reduce_merges_a_late_translation_into_the_original_row() {
    let frames = [
        r#"{"meta":{"id":"n-42","isOriginal":true,"source":"toa"},"news":{"en":["BTC up"]}}"#,
        r#"{"meta":{"id":"n-42","isOriginal":false},"news":{"en":["BTC up"],"ru":["BTC растёт"]}}"#,
    ];
    let items = reduce(frames.iter().copied());
    // Two frames, one logical row.
    assert_eq!(items.len(), 1);
    let it = &items[0];
    assert_eq!(it.en, "BTC up");
    assert_eq!(it.ru, "BTC растёт");
    // Identity/source kept from the first accepted frame; row is now an accepted translation.
    assert_eq!(it.source, "toa");
    assert!(!it.is_original);
}

#[test]
fn reduce_ignores_a_late_original_after_a_translation_was_accepted() {
    let frames = [
        r#"{"meta":{"id":"x","isOriginal":true},"news":{"en":["v1"]}}"#,
        r#"{"meta":{"id":"x","isOriginal":false},"news":{"en":["v1"],"ru":["перевод"]}}"#,
        // A late original must NOT overwrite the accepted translation.
        r#"{"meta":{"id":"x","isOriginal":true},"news":{"en":["v2"]}}"#,
    ];
    let items = reduce(frames.iter().copied());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].en, "v1");
    assert_eq!(items[0].ru, "перевод");
}

#[test]
fn reduce_keeps_first_seen_order_and_skips_invalid_frames() {
    let frames = [
        r#"{"meta":{"id":"first"},"news":{"en":["1"]}}"#,
        "garbage",
        r#"{"meta":{"id":"second"},"news":{"en":["2"]}}"#,
    ];
    let items = reduce(frames.iter().copied());
    assert_eq!(
        items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        vec!["first", "second"]
    );
}
