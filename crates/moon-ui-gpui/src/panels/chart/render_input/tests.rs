// Do not use `super::*`: the parent pulls GPUI's `test` attribute macro in through `gpui::*`,
// which would shadow the built-in `#[test]`.
use gpui::Modifiers;

use super::wheel_action;
use crate::chartdx::input::WheelAction;

/// Which wheel gesture each modifier set names, against Moonbot's own built-in list.
///
/// Moonbot: `Ctrl+Wheel` stretches the chart along time, `Ctrl+Shift+Wheel` stretches it more,
/// `Alt or Shift+Wheel` moves it left and right. A bare wheel is not on that list and zooms here.
///
/// Plausible breakage: testing the plain `Shift` pan before `Ctrl+Shift` — which is what the code
/// did until 2026-09-10, so Moonbot's coarse zoom panned the chart instead of stretching it.
#[test]
fn wheel_modifiers_name_moonbots_own_gestures() {
    let ctrl_shift = Modifiers {
        control: true,
        shift: true,
        ..Default::default()
    };

    assert_eq!(wheel_action(ctrl_shift), WheelAction::ZoomCoarse);
    assert_eq!(wheel_action(Modifiers::control()), WheelAction::Zoom);
    assert_eq!(wheel_action(Modifiers::default()), WheelAction::Zoom);
    assert_eq!(wheel_action(Modifiers::shift()), WheelAction::Pan);
    assert_eq!(wheel_action(Modifiers::alt()), WheelAction::Pan);
    // Ctrl+Alt is on nobody's list; it keeps the pan the bare Alt has, rather than becoming a
    // third zoom by accident.
    let ctrl_alt = Modifiers {
        control: true,
        alt: true,
        ..Default::default()
    };
    assert_eq!(wheel_action(ctrl_alt), WheelAction::Pan);
}
