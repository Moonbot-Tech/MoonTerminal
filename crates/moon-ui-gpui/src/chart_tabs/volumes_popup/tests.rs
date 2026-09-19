// NOT `use super::*`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use moon_core::config::{HVOL_TF_MAX_S, HvolSide};

use super::{
    PRICE_FRAME_PCTS, WIDTHS, nearest, percent_label, price_frame_label, side_of, window_choices,
};

/// The window dropdown offers `Auto`, every listed window and `Max`, in that order and nothing
/// else.
///
/// Breakage: a list that drops `Max` or reorders the windows leaves a stored value with no item
/// checked, and a press lands on a neighbour.
#[test]
fn the_window_dropdown_offers_every_window_in_order() {
    let choices: Vec<u32> = window_choices().collect();
    let listed = moon_chart::hvol::HVOL_TF_CHOICES_S;
    assert_eq!(choices.len(), listed.len() + 2);
    assert_eq!(choices[0], 0, "Auto");
    assert_eq!(&choices[1..=listed.len()], &listed[..]);
    assert_eq!(choices[listed.len() + 1], HVOL_TF_MAX_S, "Max");
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

/// The width row starts at the drawable minimum and keeps every step a stored value may hold.
///
/// Breakage: a list that starts above `WIDTH_MIN` cannot show the narrowest zone; a stored `0.1`
/// landing on a neighbour would move an existing user's zone on the first press of another row.
#[test]
fn the_width_row_starts_at_the_minimum_and_keeps_the_stored_steps() {
    assert_eq!(WIDTHS[0], moon_chart::hvol::WIDTH_MIN);
    assert_eq!(percent_label(WIDTHS[0]), "5%");
    assert_eq!(nearest(&WIDTHS, 0.05), 0);
    assert_eq!(nearest(&WIDTHS, 0.1), 1);
    assert_eq!(nearest(&WIDTHS, 0.2), 3);
    assert!(WIDTHS.windows(2).all(|w| w[0] < w[1]));
}
