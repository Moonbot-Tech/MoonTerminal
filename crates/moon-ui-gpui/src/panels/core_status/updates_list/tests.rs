use super::{from_to_text, named_build_label};
use moon_core::feed::UpdateTarget;

/// A named build shows as its short label; a name without the `MoonBot-` prefix stays whole.
///
/// Breaks when the prefix compare becomes case-sensitive or the bare prefix is cut to nothing.
#[test]
fn core_update_named_build_label_drops_the_moonbot_prefix() {
    assert_eq!(named_build_label("MoonBot-R2"), "R2");
    assert_eq!(named_build_label("moonbot-F8"), "F8");
    assert_eq!(named_build_label("SynthBuild"), "SynthBuild");
    assert_eq!(named_build_label("MoonBot-"), "MoonBot-");
    assert_eq!(named_build_label("Moon"), "Moon");
}

/// A test build installed on the release's own number names itself in the row, a Release row and
/// an `Unchanged` row do not.
///
/// Breaks when the label is dropped: the row reads `7.71 -> 7.71` and looks like nothing happened.
#[test]
fn core_update_from_to_names_an_installed_test_build() {
    let named = UpdateTarget::Named("MoonBot-R2".to_string());
    assert_eq!(
        from_to_text(Some(771), Some(771), true, &named),
        "7.71 \u{2192} 7.71 R2"
    );
    assert_eq!(
        from_to_text(Some(771), Some(771), true, &UpdateTarget::Release),
        "7.71 \u{2192} 7.71"
    );
    assert_eq!(
        from_to_text(Some(771), Some(771), false, &named),
        "7.71 \u{2192} 7.71"
    );
}
