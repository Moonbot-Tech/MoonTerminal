//! Regression coverage for the badges sharing boundary.

use super::*;

/// Regression target: deleting `parsed.custom_colors = current.custom_colors.clone();` in
/// `BadgesConfig::parse_share` imports a colleague's palette and replaces the local reuse history.
#[test]
fn parse_share_keeps_local_palette_when_pasted_text_carries_a_different_palette() {
    let local_palette = vec![[21, 22, 23], [24, 25, 26]];
    let current = BadgesConfig {
        custom_colors: local_palette.clone(),
        ..BadgesConfig::default()
    };
    let pasted = r#"{"entries": [], "custom_colors": [[201, 202, 203]]}"#;

    let parsed = BadgesConfig::parse_share(pasted, &current).expect("entries array is badges JSON");

    assert_eq!(
        parsed.custom_colors, local_palette,
        "pasting another user's badges file must not replace this user's reusable colours"
    );
}

/// Regression target: deleting `parsed.custom_colors = current.custom_colors.clone();` in
/// `BadgesConfig::parse_share` makes an older badges file silently clear the local reuse palette.
#[test]
fn parse_share_keeps_local_palette_when_pasted_text_has_no_palette_field() {
    let local_palette = vec![[31, 32, 33], [34, 35, 36]];
    let current = BadgesConfig {
        custom_colors: local_palette.clone(),
        ..BadgesConfig::default()
    };
    let pasted = r#"{"entries": []}"#;

    let parsed = BadgesConfig::parse_share(pasted, &current).expect("entries array is badges JSON");

    assert_eq!(
        parsed.custom_colors, local_palette,
        "pasting an older badges file must not erase this user's reusable colours"
    );
}

/// Regression target: removing the `entries` array validation in `BadgesConfig::parse_share`
/// accepts unrelated JSON as a badge configuration and resets Settings to serde defaults.
#[test]
fn parse_share_requires_an_entries_array_before_deserializing_badges() {
    let current = BadgesConfig::default();

    for text in ["{}", r#"{"entries": {}}"#, r#"{"other": []}"#, "not json"] {
        assert!(
            BadgesConfig::parse_share(text, &current).is_none(),
            "{text:?} is not a badges configuration because it has no entries array"
        );
    }
}
