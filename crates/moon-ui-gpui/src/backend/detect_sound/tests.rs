//! Detect-player repeat: figure alerts honour the Alerts panel, ordinary detects stay at one.

use super::detect_sound_plays;

/// Mutation: play a figure alert once regardless of the panel, or apply the panel to an ordinary
/// detect. The first restores the "repeat does nothing" bug; the second would stutter every
/// `SoundAlert=Yes` detect when the user raised the figure-alert control.
#[test]
fn figure_alerts_use_the_panel_repeat_and_ordinary_detects_play_once() {
    assert_eq!(detect_sound_plays(true, 0), 0);
    assert_eq!(detect_sound_plays(true, 2), 2);
    assert_eq!(detect_sound_plays(true, 20), 20);
    assert_eq!(detect_sound_plays(false, 0), 1);
    assert_eq!(detect_sound_plays(false, 4), 1);
}
