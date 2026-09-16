// NOT `use super::*`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use moon_core::config::{HVOL_TF_MAX_S, HvolSide};

use super::{PRICE_FRAME_PCTS, WINDOW_SEGMENTS, nearest, price_frame_label, side_of};

/// The window row's index arithmetic covers `Auto`, every listed window and `Max`, in that order.
///
/// Breakage: an off-by-one between the row's segments and the list maps a press on `12h` to `6h`,
/// or a press on `Max` to nothing.
#[test]
fn the_window_row_maps_every_segment() {
    let choices = moon_chart::hvol::HVOL_TF_CHOICES_S;
    let pick = |ix: usize| -> Option<u32> {
        match ix {
            0 => Some(0),
            i if i == choices.len() + 1 => Some(HVOL_TF_MAX_S),
            i => choices.get(i - 1).copied(),
        }
    };
    assert_eq!(pick(0), Some(0), "Auto");
    for (i, s) in choices.iter().enumerate() {
        assert_eq!(pick(i + 1), Some(*s));
    }
    assert_eq!(pick(choices.len() + 1), Some(HVOL_TF_MAX_S), "Max");
    assert_eq!(pick(choices.len() + 2), None);
    assert_eq!(WINDOW_SEGMENTS as usize, choices.len() + 2);
}

/// The two side switches compose into the four sides Moonbot lists, and back.
#[test]
fn the_side_switches_compose_into_the_four_sides() {
    for side in [
        HvolSide::Right,
        HvolSide::Left,
        HvolSide::RightTransparent,
        HvolSide::LeftTransparent,
    ] {
        assert_eq!(side_of(side.is_left(), side.is_transparent()), side);
    }
}

/// Price-window labels read as the slider's percentages, not as floating-point noise.
#[test]
fn price_frame_labels_are_trimmed_percentages() {
    assert_eq!(price_frame_label(0.1), "0.1%");
    assert_eq!(price_frame_label(0.05), "0.05%");
    assert_eq!(price_frame_label(1.0), "1%");
    assert_eq!(nearest(&PRICE_FRAME_PCTS, 0.12), 1);
    assert_eq!(nearest(&PRICE_FRAME_PCTS, f32::NAN), 0);
}
