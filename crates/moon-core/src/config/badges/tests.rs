//! Regression coverage for the badges custom-colour reuse palette and sharing boundary.

use super::*;

/// Regression target: removing the front-entry guard in `BadgesConfig::remember_custom_color`
/// duplicates the currently selected colour, making the Settings reuse palette grow on a no-op.
#[test]
fn remember_custom_color_prepends_new_values_and_leaves_the_current_front_unchanged() {
    let initial = vec![[12, 34, 56], [78, 90, 12]];
    let new_color = [210, 45, 67];
    let mut config = BadgesConfig {
        custom_colors: initial.clone(),
        ..BadgesConfig::default()
    };

    assert!(config.remember_custom_color(new_color));
    assert_eq!(
        config.custom_colors,
        vec![new_color, initial[0], initial[1]],
        "a newly typed colour must be the first reusable swatch"
    );

    let unchanged = config.custom_colors.clone();
    assert!(!config.remember_custom_color(unchanged[0]));
    assert_eq!(
        config.custom_colors, unchanged,
        "reselecting the current swatch must not duplicate or reorder the palette"
    );
}

/// Regression target: removing the existing-colour removal in `BadgesConfig::remember_custom_color`
/// leaves duplicate swatches after a previously used colour is selected again.
#[test]
fn remember_custom_color_moves_an_older_entry_to_the_front_without_a_duplicate() {
    let mut config = BadgesConfig {
        custom_colors: vec![[1, 2, 3], [4, 5, 6], [7, 8, 9], [10, 11, 12]],
        ..BadgesConfig::default()
    };

    assert!(config.remember_custom_color([7, 8, 9]));
    assert_eq!(
        config.custom_colors,
        vec![[7, 8, 9], [1, 2, 3], [4, 5, 6], [10, 11, 12]],
        "selecting an older swatch must promote that one value instead of cloning it"
    );
}

/// Regression target: removing `truncate(CUSTOM_COLORS_MAX)` in
/// `BadgesConfig::remember_custom_color` lets the persisted reuse palette exceed the picker cap.
#[test]
fn remember_custom_color_evicts_only_the_oldest_values_at_the_picker_limit() {
    let colors: Vec<[u8; 3]> = (0..=CUSTOM_COLORS_MAX as u8)
        .map(|n| [n, 255 - n, n.wrapping_mul(7)])
        .collect();
    let mut config = BadgesConfig::default();

    for color in colors.iter().copied() {
        assert!(config.remember_custom_color(color));
    }

    let expected: Vec<[u8; 3]> = colors[1..].iter().rev().copied().collect();
    assert_eq!(
        config.custom_colors, expected,
        "the newest twenty swatches must remain in recency order after the oldest is evicted"
    );
    assert_eq!(config.custom_colors.len(), CUSTOM_COLORS_MAX);
    assert!(config.custom_colors.contains(&colors[10]));
    assert!(!config.custom_colors.contains(&colors[0]));
}

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
