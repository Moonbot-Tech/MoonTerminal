// Do not use `super::*`: the parent re-exports GPUI's `test` attribute macro through its imports,
// which would shadow the built-in `#[test]`.
use super::layout::us_letter;

/// Pins what the layout translation must NOT touch.
///
/// Plausible breakage: translating an ASCII name round-trips a letter through the active layout and
/// can return a DIFFERENT letter under a non-Latin one; translating a multi-character name would
/// take the first character of `f7` or `delete` and answer for it.
#[test]
fn only_single_non_ascii_key_names_are_translated() {
    for key in ["a", "z", "s", "f7", "delete", "escape", "up", "", "1"] {
        assert_eq!(us_letter(key), None, "{key:?} must be left alone");
    }
}

/// Pins the physical-key answer on Windows when the active layout IS Latin.
///
/// Plausible breakage: a translation that fires on ASCII would rewrite every letter hotkey on the
/// developer's own machine, which is exactly where it would go unnoticed.
#[test]
fn a_latin_layout_needs_no_translation() {
    // Whatever layout this machine runs, an ASCII name is already the physical key.
    assert_eq!(us_letter("q"), None);
    assert_eq!(us_letter("w"), None);
}
