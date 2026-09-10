use super::{SCALES, step_scale};

/// The direction of Scale + and Scale −, against Moonbot's own.
///
/// Moonbot's Ctrl+Q — its "Scale +" — walks the percentage UP: 1% → 2% → 3% → 5%. So "+" grows the
/// scale NUMBER and widens the visible price band, which is zooming OUT. The terminal read "+" as
/// "zoom in" and stepped the other way, so both shortcuts did the opposite of the label on the row,
/// and nothing said so: the only readers are the two hotkey handlers and no test pinned this.
///
/// Plausible breakage: flipping the branch back, or renaming the parameter to `zoom_in` without
/// flipping the call sites with it.
#[test]
fn scale_plus_widens_the_band_as_moonbot_does() {
    // 5% is one step wider than 2%, and Auto is the widest of all.
    assert_eq!(step_scale(Some(0.02), true), Some(0.05));
    assert_eq!(step_scale(Some(0.05), true), Some(0.10));
    assert_eq!(
        step_scale(Some(0.50), true),
        None,
        "Auto is the far end of +"
    );

    // And Scale − narrows it, down to the tightest preset.
    assert_eq!(step_scale(None, false), Some(0.50));
    assert_eq!(step_scale(Some(0.05), false), Some(0.02));
}

/// Both ends clamp instead of wrapping: a shortcut held down must stop, not jump from the tightest
/// zoom back out to Auto.
#[test]
fn the_ends_clamp() {
    let widest = SCALES[0].1;
    let tightest = SCALES[SCALES.len() - 1].1;

    assert_eq!(step_scale(widest, true), widest);
    assert_eq!(step_scale(tightest, false), tightest);
}

/// A scale dragged to a value that is not a preset steps from its NEAREST one, so a custom zoom
/// does not silently jump to the end of the list on the first press.
#[test]
fn a_custom_value_steps_from_its_nearest_preset() {
    // 0.09 sits nearest 10%; one step wider is 20%.
    assert_eq!(step_scale(Some(0.09), true), Some(0.20));
    // and one step tighter is 5%.
    assert_eq!(step_scale(Some(0.09), false), Some(0.05));
}
