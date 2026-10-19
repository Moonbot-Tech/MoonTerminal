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
fn tag_entity_dedups_and_strips_multiple_hashes() {
    let item = parse_frame(
        r###"{"meta":{"id":"d"},"news":{"en":["x"]},
            "tags":{"entity":[{"text":"##listing"},{"text":"#Listing"},{"text":"# spaced "}]}}"###,
    )
    .unwrap();
    // "##listing" -> "listing"; "#Listing" dedups by case-folded key; "# spaced " -> "spaced".
    assert_eq!(item.tags, vec!["listing", "spaced"]);
}

#[test]
fn parse_catalog_strips_hash_and_dedups_case_insensitively() {
    let cat = parse_catalog(Some(
        r##"{"count":3,"tags":[{"context":"ENTITY","name":"#listing","id":1},
            {"name":"#Hack"},{"name":"#listing"},{"name":""}]}"##,
    ));
    assert_eq!(cat, vec!["listing", "Hack"]);
    // No catalog / bad JSON yields an empty vocabulary, not a panic.
    assert!(parse_catalog(None).is_empty());
    assert!(parse_catalog(Some("not json")).is_empty());
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
    let item =
        parse_frame(r#"{"meta":{"id":"a"},"news":{"en":["line one\r\nline two","\n three "]}}"#)
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

/// Ukrainian, Turkish, Portuguese, and Vietnamese have no news translation.
/// Returning Russian, Spanish, or a blank body would show the wrong language
/// on the card and on the clipboard.
#[test]
fn uk_body_falls_back_to_english() {
    let item = NewsItem {
        en: "BTC up".to_string(),
        ru: "BTC rastet".to_string(),
        es: "BTC sube".to_string(),
        ..NewsItem::default()
    };
    assert_eq!(item.body(crate::config::Language::Uk), "BTC up");
    assert_eq!(item.body(crate::config::Language::Tr), "BTC up");
    assert_eq!(item.body(crate::config::Language::Pt), "BTC up");
    assert_eq!(item.body(crate::config::Language::Vi), "BTC up");
}

/// Catches reading the wrong key or a wrong default: the key is sent only when true, so an absent
/// key must leave the item waiting for a translation.
#[test]
fn parse_frame_reads_no_translate_only_when_present() {
    let flagged =
        parse_frame(r#"{"meta":{"id":"k","noTranslate":true},"news":{"en":["x"]}}"#).unwrap();
    assert!(flagged.no_translate);
    let plain = parse_frame(r#"{"meta":{"id":"k"},"news":{"en":["x"]}}"#).unwrap();
    assert!(!plain.no_translate);
}

/// Catches `merge_from` dropping the flag: the "no translation" verdict for a failed translation
/// arrives as a second frame of the same id, after the first frame already made the row.
#[test]
fn reduce_takes_no_translate_from_a_later_frame() {
    let frames = [
        r#"{"meta":{"id":"n","isOriginal":true},"news":{"en":["Kucoin API closed withdrawals"]}}"#,
        r#"{"meta":{"id":"n","isOriginal":true,"noTranslate":true},"news":{"en":["Kucoin API closed withdrawals"]}}"#,
    ];
    let items = reduce(frames.iter().copied());
    assert_eq!(items.len(), 1);
    assert!(items[0].no_translate);
    assert_eq!(items[0].en, "Kucoin API closed withdrawals");
}

/// Catches assigning the flag instead of OR-ing it: a late ring copy without the key would bring
/// the "translation pending" badge back.
#[test]
fn reduce_keeps_no_translate_after_a_later_frame_without_it() {
    let frames = [
        r#"{"meta":{"id":"n","isOriginal":true,"noTranslate":true},"news":{"en":["x"]}}"#,
        r#"{"meta":{"id":"n","isOriginal":true},"news":{"en":["x"]}}"#,
    ];
    let items = reduce(frames.iter().copied());
    assert!(items[0].no_translate);
}

/// Catches the badge predicate ignoring the flag (the forever-pending badge) or ignoring `ru`.
#[test]
fn awaits_translation_until_ru_arrives_or_the_service_says_none() {
    let waiting = NewsItem {
        en: "x".to_string(),
        ..NewsItem::default()
    };
    assert!(waiting.awaits_translation());
    let final_en = NewsItem {
        no_translate: true,
        ..waiting.clone()
    };
    assert!(!final_en.awaits_translation());
    let translated = NewsItem {
        ru: "перевод".to_string(),
        ..waiting
    };
    assert!(!translated.awaits_translation());
}

/// Catches a cross-core merge that keeps the first core's copy: NewsService marks every frame
/// `isOriginal: true`, so the old "prefer `is_original == false`" rule never picked the `ru` copy.
#[test]
fn merge_core_copy_prefers_ru_when_both_copies_claim_original() {
    let mut first = NewsItem {
        id: "n".to_string(),
        en: "x".to_string(),
        is_original: true,
        ..NewsItem::default()
    };
    let translated = NewsItem {
        ru: "перевод".to_string(),
        ..first.clone()
    };
    assert!(first.merge_core_copy(&translated));
    assert_eq!(first.ru, "перевод");
    // The reverse must not drop the translation for a copy without one.
    let bare = NewsItem {
        ru: String::new(),
        ..first.clone()
    };
    assert!(!first.merge_core_copy(&bare));
    assert_eq!(first.ru, "перевод");
}

/// Catches a merge that keeps the first core's copy whole: core A saw only the first frame, core B
/// also saw the `noTranslate` frame, and the row must stop waiting either way round.
#[test]
fn merge_core_copy_unions_no_translate() {
    let plain = NewsItem {
        id: "n".to_string(),
        en: "x".to_string(),
        is_original: true,
        ..NewsItem::default()
    };
    let flagged = NewsItem {
        no_translate: true,
        ..plain.clone()
    };
    let mut a = plain.clone();
    a.merge_core_copy(&flagged);
    assert!(a.no_translate);
    let mut b = flagged;
    b.merge_core_copy(&plain);
    assert!(b.no_translate);
}

/// Catches the reducer's translation lock swallowing the flag: once a row is an accepted
/// translation (`isOriginal: false`) its texts are locked, but a `noTranslate` frame for it must
/// still stop the wait when that translation carried no `ru`.
#[test]
fn reduce_takes_no_translate_into_a_locked_translation_row() {
    let frames = [
        r#"{"meta":{"id":"n","isOriginal":true},"news":{"en":["x"]}}"#,
        r#"{"meta":{"id":"n","isOriginal":false},"news":{"en":["x"],"es":["x es"]}}"#,
        r#"{"meta":{"id":"n","isOriginal":true,"noTranslate":true},"news":{"en":["late"]}}"#,
    ];
    let items = reduce(frames.iter().copied());
    assert!(items[0].no_translate);
    // The lock still holds for the texts.
    assert_eq!(items[0].en, "x");
}
