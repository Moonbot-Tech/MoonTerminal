use super::{from_to_text, live_from_to};
use moon_core::feed::{UpdateTarget, named_build_label};

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
        from_to_text(Some(771), None, Some(771), None, true, &named),
        "7.71 \u{2192} 7.71 R2"
    );
    assert_eq!(
        from_to_text(
            Some(771),
            None,
            Some(771),
            None,
            true,
            &UpdateTarget::Release
        ),
        "7.71 \u{2192} 7.71"
    );
    assert_eq!(
        from_to_text(Some(771), None, Some(771), None, false, &named),
        "7.71 \u{2192} 7.71"
    );
}

/// A history row prints the letter the outcome stored, and does not append the target label
/// on top of it.
///
/// Breaks when both are printed: the cell reads `7.71 -> 7.71 R3 R3` and names the build twice.
#[test]
fn history_row_prints_the_outcome_letter_once() {
    let named = UpdateTarget::Named("MoonBot-R3".to_string());
    assert_eq!(
        from_to_text(Some(771), None, Some(771), Some("R3"), true, &named),
        "7.71 \u{2192} 7.71 R3"
    );
    assert_eq!(
        from_to_text(
            Some(771),
            None,
            Some(771),
            Some("R2"),
            false,
            &UpdateTarget::Named("MoonBot-R2".to_string())
        ),
        "7.71 \u{2192} 7.71 R2"
    );
}

/// An in-flight Waiting row shows the baseline letter captured at send.
///
/// Breaks when the phase letter is dropped: the cell reads `7.71 -> ...` while the core that
/// left was `7.71 R2`.
#[test]
fn in_flight_waiting_shows_the_baseline_letter() {
    assert_eq!(
        live_from_to(Some(771), Some("R2")),
        "7.71 R2 \u{2192} \u{2026}"
    );
    assert_eq!(live_from_to(Some(771), None), "7.71 \u{2192} \u{2026}");
}
