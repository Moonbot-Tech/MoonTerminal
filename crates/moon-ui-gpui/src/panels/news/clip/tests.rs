// `use super::*` is banned here: the parent chain re-exports `gpui::*`, whose own `test` attribute
// shadows the built-in one and makes `#[test]` expand recursively.
use crate::panels::news::clip::telegram_text;
use moon_core::feed::NewsItem;

/// 2026-07-25 08:26:50 UTC.
const TIME_MS: i64 = 1_784_968_010_000;

fn item() -> NewsItem {
    NewsItem {
        time_ms: TIME_MS,
        source: "toa".to_string(),
        coins: vec!["vanry".to_string(), "portal".to_string()],
        tags: vec!["listing".to_string()],
        en: "English body".to_string(),
        ru: "Русский текст".to_string(),
        ..NewsItem::default()
    }
}

/// Catches writing the tickers or topics as bare words: Telegram linkifies `$COIN` and `#topic`,
/// and that linkification is the whole reason this is not just the card's text.
///
/// Also pins the layout: tickers ride with the header, topics file the item from underneath the
/// body. Putting the hashtags back between the headline and the text is the regression here.
#[test]
fn tickers_head_the_post_and_topics_close_it() {
    let text = telegram_text(&item(), true);
    assert_eq!(
        text,
        "TOA · 25.07.2026 08:26:50 UTC\n$VANRY $PORTAL\n\nРусский текст\n#listing"
    );
}

/// Catches pasting the absolute stamp as the card's relative age: a forwarded message is read
/// later, when "16 сек назад" means something else entirely.
#[test]
fn the_stamp_is_absolute_utc() {
    let text = telegram_text(&item(), true);
    assert!(
        text.starts_with("TOA · 25.07.2026 08:26:50 UTC\n"),
        "{text}"
    );
}

/// Catches copying a language the user was not looking at: the toggle that picks the card's body
/// must pick the clipboard's body too.
#[test]
fn the_body_follows_the_cards_translate_toggle() {
    assert!(telegram_text(&item(), true).contains("\nРусский текст\n"));
    assert!(telegram_text(&item(), false).contains("\nEnglish body\n"));
}

/// Catches emitting stray separators for an item with no source, no stamp and no subjects — the
/// shape a malformed frame arrives in.
#[test]
fn a_bare_item_yields_just_its_body() {
    let bare = NewsItem {
        en: "Body only".to_string(),
        ..NewsItem::default()
    };
    assert_eq!(telegram_text(&bare, false), "Body only");
}

/// Catches adding the two service offsets together: both are measured from publication, so their
/// sum is not a duration at all — the delay is the later mark, here the send at +2349.
#[test]
fn the_delay_is_the_latest_service_mark_not_the_sum() {
    let timed = NewsItem {
        time_ms: TIME_MS,
        recv_time_ms: Some(TIME_MS + 6),
        send_time_ms: Some(TIME_MS + 2_349),
        en: "x".to_string(),
        ..NewsItem::default()
    };
    let text = telegram_text(&timed, false);
    assert!(text.contains("(+2349"), "{text}");
    assert!(!text.contains("2355"), "{text}");
}

/// Catches folding the terminal receipt into the delay: it is stamped by this PC's clock, and on a
/// reconnect backfill it is the moment of connect, which would report minutes of "delay".
#[test]
fn the_terminal_receipt_is_not_part_of_the_delay() {
    let timed = NewsItem {
        time_ms: TIME_MS,
        recv_terminal_ms: Some(TIME_MS + 800_000),
        en: "x".to_string(),
        ..NewsItem::default()
    };
    let text = telegram_text(&timed, false);
    assert_eq!(text, "25.07.2026 08:26:50 UTC\n\nx");
}

/// Catches double-prefixing a tag that already carries its `#`.
#[test]
fn a_tag_is_not_hashed_twice() {
    let tagged = NewsItem {
        tags: vec!["#hack".to_string()],
        en: "x".to_string(),
        ..NewsItem::default()
    };
    assert!(telegram_text(&tagged, false).contains("#hack"));
    assert!(!telegram_text(&tagged, false).contains("##"));
}
