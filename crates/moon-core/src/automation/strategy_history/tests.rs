//! What a version list shows of a diff.

use super::*;

/// Regression target: returning a changed coin list whole, kilobytes per version.
#[test]
fn a_long_changed_value_is_shown_as_its_size() {
    let list = (0..80)
        .map(|i| format!("COIN{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let raw = json!({"CoinsBlackList":{"old":"A,B","new":list},"StopLoss":{"old":-1.5,"new":-2}})
        .to_string();
    let diff = changed(Some(&raw));
    assert_eq!(diff["StopLoss"], json!({"old":-1.5,"new":-2}));
    assert_eq!(diff["CoinsBlackList"]["old"], "A,B");
    assert_eq!(diff["CoinsBlackList"]["new"]["items"], 80);
    assert!(changed(None).is_null());
    assert!(changed(Some("not json")).is_null());
}

/// A coin list separated by spaces, as MoonBot accepts it (#1448), counts each coin, not one.
#[test]
fn a_space_separated_list_counts_each_coin() {
    let list = (0..60)
        .map(|i| format!("COIN{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let raw = json!({"CoinsBlackList":{"old":"A B","new":format!("ANC GALA {list}")}}).to_string();
    let diff = changed(Some(&raw));
    assert_eq!(diff["CoinsBlackList"]["new"]["items"], 62);
}

/// A long free-text field keeps the comma count it always had: a comment of words with one comma
/// is two items, not a word count (#1448 changed coin lists only).
#[test]
fn a_long_comment_keeps_its_comma_count() {
    let comment = "watch the big green candles before the night session opens on the main venues, and do not chase anything that has already doubled today or tripled this week on thin volume";
    assert!(comment.chars().count() > LONG_VALUE);
    let raw = json!({"Comment":{"old":"x","new":comment}}).to_string();
    let diff = changed(Some(&raw));
    assert_eq!(diff["Comment"]["new"]["items"], 2);
}
