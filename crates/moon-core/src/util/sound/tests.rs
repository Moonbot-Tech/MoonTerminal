//! Regression coverage for sound matching across archive folder spellings.

use super::sound_stem;

/// Removing path-tail extraction breaks Moonbot archive names and causes default playback.
#[test]
fn sound_names_match_the_flat_file_stem() {
    for name in [
        "sounds/hook",
        r"sounds\hook",
        "Sounds/Hook.wav",
        " hook.wav ",
        "hook",
        r"outer/inner\Hook.WAV",
    ] {
        assert_eq!(sound_stem(name), "hook", "{name}");
    }
    for name in ["sounds/", r"sounds\", "", "  ", ".wav"] {
        assert_eq!(sound_stem(name), "", "{name}");
    }
}
