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
