//! Track-cap math for the shared settings slider row.
//!
//! Breakage: dropping the caption floor lets a long endpoint pair overflow the track; replacing
//! the height multiple with a window-sized number stretches every settings slider across a wide
//! pane.

// NOT `use super::*;`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use super::{SLIDER_TRACK_HEIGHTS, slider_track_max_w};

/// Twelve 22 design-px slider heights is 264, the comfortable track beside the 260-wide valuation
/// select on the same General form. Captions wider than that floor win so the endpoints cannot
/// collide.
#[test]
fn slider_track_uses_caption_floor_or_twelve_heights() {
    assert_eq!(SLIDER_TRACK_HEIGHTS, 12.0);
    let slider_h = 22.0;
    assert_eq!(slider_track_max_w(40.0, slider_h), 264.0);
    assert_eq!(slider_track_max_w(264.0, slider_h), 264.0);
    assert_eq!(slider_track_max_w(272.0, slider_h), 272.0);
}

/// Count labels take the plural form of each language: one/few/many in Russian and Ukrainian
/// (21 is singular there), one/other everywhere else (21 is plural in English).
#[test]
fn count_labels_take_each_languages_plural_form() {
    use super::count_label;
    let cases: [(&str, [&str; 4]); 3] = [
        ("ru", ["1 ядро", "2 ядра", "5 ядер", "21 ядро"]),
        ("uk", ["1 ядро", "2 ядра", "5 ядер", "21 ядро"]),
        ("en", ["1 core", "2 cores", "5 cores", "21 cores"]),
    ];
    for (lang, want) in cases {
        let _locale = crate::test_locale::force(lang);
        for (n, want) in [1, 2, 5, 21].into_iter().zip(want) {
            assert_eq!(count_label("conn.member_count", n), want, "{lang} {n}");
        }
    }
    {
        let _locale = crate::test_locale::force("en");
        assert_eq!(count_label("hotkeys.banner.conflicts", 21), "21 conflicts.");
    }
    let _locale = crate::test_locale::force("uk");
    assert_eq!(count_label("hotkeys.banner.conflicts", 21), "21 конфлікт.");
    assert_eq!(
        count_label("hotkeys.banner.conflicts", 12),
        "12 конфліктів."
    );
}
