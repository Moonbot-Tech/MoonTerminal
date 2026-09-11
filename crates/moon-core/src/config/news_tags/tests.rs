use super::NewsTagSettings;

/// Stored symbolic choices stay symbolic across a save/reload while fixed RGB and filters survive.
#[test]
fn symbolic_and_custom_colors_roundtrip_without_rewriting_legacy_choices() {
    let original = r##"{"colors":{"hack":"red","listing":"#12aBcD","black":"#000000","future":"future-color"},"hidden":["muted"],"hide_untagged":true}"##;
    let mut settings: NewsTagSettings = serde_json::from_str(original).expect("valid legacy JSON");
    assert!(settings.set_color("new", Some("#89ABEF")));
    let saved = serde_json::to_string(&settings).expect("serializable settings");
    let loaded: NewsTagSettings = serde_json::from_str(&saved).expect("reload saved settings");
    assert_eq!(loaded.color("hack"), Some("red"));
    assert_eq!(loaded.color("listing"), Some("#12aBcD"));
    assert_eq!(loaded.color("black"), Some("#000000"));
    assert_eq!(loaded.color("future"), Some("future-color"));
    assert_eq!(loaded.color("new"), Some("#89ABEF"));
    assert!(loaded.is_hidden("muted"));
    assert!(loaded.hide_untagged());
    assert_eq!(loaded.rev(), 0, "runtime revision must not be persisted");
}

/// Rejecting shorthand/alpha prevents partially typed or unrelated symbolic values becoming RGB.
#[test]
fn fixed_rgb_requires_exact_six_digit_representation() {
    assert_eq!(NewsTagSettings::custom_rgb("#12aBcD"), Some(0x12abcd));
    assert_eq!(NewsTagSettings::custom_rgb("#000000"), Some(0));
    for value in [
        "red",
        "123456",
        "#123",
        "#12345678",
        "#12345g",
        "#１２３４５６",
    ] {
        assert_eq!(NewsTagSettings::custom_rgb(value), None, "{value}");
    }
}

/// Switching to neutral must remove the serialized override and trigger the existing repaint path.
#[test]
fn clearing_custom_color_removes_override_and_only_real_changes_bump_revision() {
    let mut settings: NewsTagSettings =
        serde_json::from_str(r##"{"colors":{"listing":"#12ABCD"}}"##).expect("valid JSON");
    assert!(!settings.set_color("listing", Some("#12ABCD")));
    assert_eq!(settings.rev(), 0);
    assert!(settings.set_color("listing", None));
    assert_eq!(settings.rev(), 1);
    assert!(!settings.set_color("listing", None));
    assert_eq!(settings.rev(), 1);
    let saved = serde_json::to_value(&settings).expect("serializable settings");
    assert!(saved["colors"].as_object().expect("color map").is_empty());
}
