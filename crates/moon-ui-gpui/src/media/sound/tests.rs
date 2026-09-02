//! The ordinal-to-sound table: the one place a wrong index plays the wrong file.

use super::{MB_SOUNDS, SOUNDS, mb_sound_name};

/// Every name Moonbot offers resolves to a sound this binary actually carries, and the two lists
/// hold exactly the same set.
///
/// Plausible breakage: a sound added to `SOUNDS` but not to `MB_SOUNDS` is unreachable from the
/// core's setting; one added to `MB_SOUNDS` alone shifts every ordinal after it onto a different
/// file, silently re-pointing a setting already stored on the core.
#[test]
fn every_moonbot_sound_is_one_this_build_can_play() {
    assert_eq!(
        MB_SOUNDS.len(),
        SOUNDS.len(),
        "the two sound lists must describe the same set"
    );
    for label in MB_SOUNDS {
        let stem = label.to_ascii_lowercase();
        assert!(
            SOUNDS.iter().any(|(s, _)| *s == stem),
            "Moonbot lists {label}, which no embedded sound answers to"
        );
    }
    for (stem, _) in SOUNDS {
        assert!(
            MB_SOUNDS.iter().any(|l| l.to_ascii_lowercase() == *stem),
            "{stem} is embedded but absent from Moonbot's list, so no ordinal reaches it"
        );
    }
}

/// The ordinal is 1-BASED, as the protocol's own field doc states.
///
/// Plausible breakage: a 0-based read shows — and writes back — the neighbouring sound, which no
/// type can catch and which the user only notices as "the wrong thing beeped".
#[test]
fn the_ordinal_is_one_based_and_round_trips() {
    assert_eq!(mb_sound_name(1), Some("Alarm"));
    // The two the developer's own Moonbot had selected when this landed, which is what makes them
    // worth pinning: the sell row showed PFIFF and the buy row HALLO.
    assert_eq!(mb_sound_name(7), Some("PFIFF"));
    assert_eq!(mb_sound_name(6), Some("HALLO"));
    assert_eq!(
        mb_sound_name(MB_SOUNDS.len() as i32),
        Some("ComeGetSome"),
        "the last ordinal must reach the last entry, not fall off it"
    );
    for (i, label) in MB_SOUNDS.iter().enumerate() {
        assert_eq!(
            mb_sound_name(i as i32 + 1),
            Some(*label),
            "the picker builds its ordinal as index + 1; this is the reader agreeing"
        );
    }
}

/// An ordinal outside the list names nothing rather than falling back to the first sound.
///
/// Plausible breakage: clamping an unknown ordinal to `Alarm` makes the popup show a sound the core
/// never held, and the next OK writes that guess into the core's config.
#[test]
fn an_ordinal_outside_the_list_names_nothing() {
    assert_eq!(mb_sound_name(0), None, "zero is below a 1-based list");
    assert_eq!(mb_sound_name(-1), None);
    assert_eq!(mb_sound_name(i32::MIN), None, "the -1 must not overflow");
    assert_eq!(mb_sound_name(MB_SOUNDS.len() as i32 + 1), None);
}
