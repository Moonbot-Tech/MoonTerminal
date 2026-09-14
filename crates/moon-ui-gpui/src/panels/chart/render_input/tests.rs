// Do not use `super::*`: the parent pulls GPUI's `test` attribute macro in through `gpui::*`,
// which would shadow the built-in `#[test]`.
use gpui::Modifiers;

use super::wheel_delta;

/// The wheel's movement has to be read off whichever axis the platform filed it under.
///
/// Windows moves a Shift+wheel onto X and leaves Y at zero — its own "Shift means horizontal"
/// convention, faithfully mirrored by the fork. The chart read Y alone and bailed on a zero delta,
/// so Shift+wheel panning did nothing at all on Windows while the built-in list, the tour and the
/// settings page all promised it. Alt+wheel took the same code path with Y intact, which is why the
/// broken half stayed hidden behind a caption that names both.
///
/// Plausible breakage: reading `y` unconditionally again, or taking `x` without the Shift test —
/// which would hand a real horizontal wheel to the zoom.
#[test]
fn a_shift_wheel_is_read_off_the_axis_windows_files_it_under() {
    assert_eq!(wheel_delta(0.0, 3.0, Modifiers::default()), 3.0);
    assert_eq!(wheel_delta(0.0, -3.0, Modifiers::default()), -3.0);
    // Shift: the platform moved the same value, sign and all, onto X.
    assert_eq!(wheel_delta(3.0, 0.0, Modifiers::shift()), 3.0);
    assert_eq!(wheel_delta(-3.0, 0.0, Modifiers::shift()), -3.0);
    // Alt keeps Y, and a horizontal wheel without Shift is not the vertical one.
    assert_eq!(wheel_delta(0.0, 3.0, Modifiers::alt()), 3.0);
    assert_eq!(wheel_delta(3.0, 0.0, Modifiers::default()), 0.0);
    // A Shift gesture that really did move Y keeps Y, so the fallback cannot steal it.
    assert_eq!(wheel_delta(9.0, 3.0, Modifiers::shift()), 3.0);
}

/// Checking Shift pan before Ctrl+Shift turns super zoom into a horizontal pan on Windows.
#[test]
fn super_zoom_routes_windows_shift_delta_before_pan() {
    use super::wheel_mode;
    use crate::chartdx::input::WheelMode;
    let super_mods = Modifiers {
        control: true,
        shift: true,
        ..Modifiers::default()
    };
    assert_eq!(wheel_mode(super_mods), WheelMode::SuperZoom);
    assert_eq!(wheel_delta(3.0, 0.0, super_mods), 3.0);
    assert_eq!(wheel_delta(-3.0, 0.0, super_mods), -3.0);
    assert_eq!(wheel_mode(Modifiers::shift()), WheelMode::Pan);
    assert_eq!(wheel_mode(Modifiers::alt()), WheelMode::Pan);
    assert_eq!(wheel_mode(Modifiers::default()), WheelMode::Zoom);
}
