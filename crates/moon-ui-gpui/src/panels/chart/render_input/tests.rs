// Do not use `super::*`: the parent pulls GPUI's `test` attribute macro in through `gpui::*`,
// which would shadow the built-in `#[test]`.
use gpui::Modifiers;

use super::sells_zone_claims_press;
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

/// Sells-to-zone left-press withholding is (armed, modifiers) only. A pane argument
/// would let a Ctrl+Left that missed the plot — the order book under the default
/// `separate_control_zones = true` — fall through to `sell_move_click = LeftCtrl`
/// and reprice live orders.
///
/// Plausible breakage: adding a pane/surface parameter to `sells_zone_claims_press`.
#[test]
fn sells_zone_claims_press_has_no_pane_parameter() {
    let src = include_str!("../render_input.rs");
    let start = src
        .find("fn sells_zone_claims_press(")
        .expect("sells_zone_claims_press must exist");
    let sig = &src[start..];
    let sig_end = sig.find(')').expect("signature closes");
    let sig = &sig[..sig_end];
    assert!(
        sig.contains("armed: bool") && sig.contains("modifiers: Modifiers"),
        "the decision is (armed, modifiers)"
    );
    assert!(
        !sig.contains("pane") && !sig.contains("surface"),
        "a pane parameter lets a missed-plot Ctrl+Left reprice live orders"
    );
}

/// Mode off means no band press, including a raw Ctrl that would otherwise match
/// default Move TP. The oracle is the mode flag, not a modifier table.
///
/// Plausible breakage: dropping the `armed &&` conjunct so a modifier alone claims
/// the press while the mode is off.
#[test]
fn sells_zone_claims_press_unarmed_never_claims() {
    for modifiers in [
        Modifiers::none(),
        Modifiers::control(),
        Modifiers::secondary_key(),
        Modifiers::shift(),
        Modifiers::alt(),
        Modifiers::command(),
    ] {
        assert!(
            !sells_zone_claims_press(false, modifiers),
            "unarmed must not claim {modifiers:?}"
        );
    }
}

/// Issue #668: with the mode armed, an unmodified left press still places in the
/// order book and still enters a coin by market. The original wide `!sells_zone_mode`
/// gate swallowed every left press.
///
/// Plausible breakage: `render_input.rs:sells_zone_claims_press` becoming `armed`
/// alone, so every left press is withheld again while the mode runs.
#[test]
fn sells_zone_claims_press_leaves_unmodified_left_for_the_book() {
    assert!(!sells_zone_claims_press(true, Modifiers::none()));
}

/// Shift and Alt are other trading modifiers (`buy_move_click = LeftShift`, Alt
/// bindings), not the band's Ctrl/Cmd press. Armed mode must leave them to trading.
///
/// Plausible breakage: claiming every modified press, or treating shift/alt as
/// secondary, so Move Open / Alt bindings die while the mode is on.
#[test]
fn sells_zone_claims_press_leaves_shift_and_alt_to_trading() {
    assert!(!sells_zone_claims_press(true, Modifiers::shift()));
    assert!(!sells_zone_claims_press(true, Modifiers::alt()));
}

/// Raw Control must claim on every platform. `gesture_matches` LeftCtrl reads
/// `modifiers.control` directly and never `platform`; on macOS `secondary()` is
/// Command, so `armed && modifiers.secondary()` lets a physical Ctrl+Left through
/// to default Move TP and reprices live orders.
///
/// The behavioral assert holds on Windows too (Control IS secondary there). The
/// source pin against `trade.rs`'s LeftCtrl arm is what reddens the macOS-only
/// drop of the `modifiers.control` disjunct on this host.
///
/// Plausible breakage: `armed && (modifiers.control || modifiers.secondary())`
/// becoming `armed && modifiers.secondary()`.
#[test]
fn sells_zone_claims_press_raw_control_on_every_platform() {
    assert!(sells_zone_claims_press(true, Modifiers::control()));
    let trade = include_str!("../trade.rs");
    assert!(
        trade.contains("MouseGestureBinding::LeftCtrl =>") && trade.contains("modifiers.control"),
        "LeftCtrl matches the raw control bit; the band must withhold that same bit"
    );
    let src = include_str!("../render_input.rs");
    let start = src
        .find("fn sells_zone_claims_press(")
        .expect("sells_zone_claims_press must exist");
    let rest = &src[start..];
    let open = rest.find('{').expect("body opens");
    let close = rest[open..].find('}').expect("body closes");
    let body = &rest[open..open + close];
    assert!(
        body.contains("modifiers.control"),
        "dropping modifiers.control lets macOS Ctrl+Left through to Move TP"
    );
}

/// The band's own modifier is the platform secondary key (Ctrl off-macOS, Cmd on
/// macOS). Armed + that key is the press that draws the rectangle.
///
/// Plausible breakage: ignoring `secondary()` so Cmd+Left on macOS places or
/// moves orders instead of drawing the band.
#[test]
fn sells_zone_claims_press_secondary_key_is_the_band() {
    assert!(sells_zone_claims_press(true, Modifiers::secondary_key()));
}

/// Control+Shift still carries the control bit that LeftCtrl matches, so the band
/// still claims. Super-zoom on the wheel is a different gesture; this press must
/// not leak a Move TP.
///
/// Plausible breakage: requiring an exact modifier match (control and not shift)
/// so Ctrl+Shift+Left fires the default Move TP while the mode is armed.
#[test]
fn sells_zone_claims_press_control_and_shift_still_claims() {
    let control_shift = Modifiers {
        control: true,
        shift: true,
        ..Modifiers::default()
    };
    assert!(sells_zone_claims_press(true, control_shift));
}
