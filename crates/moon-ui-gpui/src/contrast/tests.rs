//! Unit coverage for the shared contrast arithmetic and the lightness lift built on it.

use super::{contrast_ratio, lift_to_contrast, mix, pole, relative_luminance};
use crate::design::{hsla_to_rgb8, picker_palette, rgb_bytes_to_hsla};

const WHITE: [u8; 3] = [255, 255, 255];
/// `ChartTheme::default().bg`, the shipped dark chart ground.
const DARK: [u8; 3] = [30, 30, 30];
/// The non-text floor both core-colour consumers hold.
const FLOOR: f64 = 3.0;

/// `contrast.rs:lift_to_contrast` must return a readable pick untouched; otherwise a user who chose
/// a colour that already works sees it drift after this change, which is the one regression the
/// lift is not allowed to introduce.
#[test]
fn lift_leaves_a_readable_colour_byte_for_byte() {
    assert_eq!(lift_to_contrast([0, 0, 255], WHITE, FLOOR), [0, 0, 255]);
    assert_eq!(lift_to_contrast([0, 255, 0], DARK, FLOOR), [0, 255, 0]);
    assert_eq!(
        lift_to_contrast([90, 90, 90], [0, 0, 0], FLOOR),
        [90, 90, 90]
    );
}

/// `contrast.rs:lift_to_contrast` must darken a yellow on white without rotating it; otherwise the
/// Binance core's frame stops being yellow at all, which is the fallback-to-accent this replaces.
#[test]
fn lift_darkens_yellow_on_white_and_keeps_its_hue() {
    let picked = [0xF3, 0xBA, 0x2F];
    let lifted = lift_to_contrast(picked, WHITE, FLOOR);

    assert!(contrast_ratio(lifted, WHITE) >= FLOOR);
    assert_ne!(lifted, picked);
    let (before, after) = (rgb_bytes_to_hsla(picked), rgb_bytes_to_hsla(lifted));
    assert!(
        (before.h - after.h).abs() < 0.01,
        "hue moved {} -> {}",
        before.h,
        after.h
    );
    assert!(after.l < before.l);
}

/// `contrast.rs:lift_to_contrast` must lighten a navy on the dark theme without greying it;
/// otherwise an RGB blend toward white would satisfy the floor with a lilac nobody recognises.
#[test]
fn lift_lightens_navy_on_dark_and_keeps_its_saturation() {
    let picked = [0x20, 0x20, 0x40];
    let lifted = lift_to_contrast(picked, DARK, FLOOR);

    assert!(contrast_ratio(lifted, DARK) >= FLOOR);
    let (before, after) = (rgb_bytes_to_hsla(picked), rgb_bytes_to_hsla(lifted));
    assert!((before.h - after.h).abs() < 0.01);
    assert!(after.l > before.l);
    assert!(
        after.s >= before.s - 0.05,
        "saturation fell {} -> {}",
        before.s,
        after.s
    );
    // Blue stays the dominant channel: the result is a blue, not a grey.
    assert!(lifted[2] > lifted[0] + 40 && lifted[2] > lifted[1] + 40);
}

/// `contrast.rs:lift_to_contrast` must stop at the first readable step; otherwise a colour that
/// barely fails is pushed far past the floor and the frame reads as a different colour than the
/// Detects card that stayed close to the pick.
#[test]
fn lift_stops_just_past_the_floor() {
    for (color, ground) in [([0xF4, 0x7B, 0x7B], WHITE), ([0xA0, 0x0D, 0x0D], DARK)] {
        let lifted = lift_to_contrast(color, ground, FLOOR);
        let ratio = contrast_ratio(lifted, ground);
        assert!(
            (FLOOR..FLOOR + 0.35).contains(&ratio),
            "overshot to {ratio}"
        );
    }
}

/// `contrast.rs:lift_to_contrast` must reach the floor for every picker swatch on both shipped
/// grounds; otherwise the palette the picker offers still contains choices the lift cannot rescue,
/// and those cores would need a fallback again.
#[test]
fn every_picker_swatch_reaches_the_floor_on_both_shipped_grounds() {
    for swatch in picker_palette() {
        let color = hsla_to_rgb8(swatch);
        for ground in [WHITE, DARK] {
            let lifted = lift_to_contrast(color, ground, FLOOR);
            assert!(
                contrast_ratio(lifted, ground) >= FLOOR,
                "{color:?} on {ground:?} lifted to {lifted:?}"
            );
        }
    }
}

/// `contrast.rs:lift_to_contrast` must be total on a ground identical to the colour and on the
/// crossover grey; otherwise a user-configured theme can hand the arrival border a colour with no
/// contrast at all.
#[test]
fn lift_is_total_on_degenerate_grounds() {
    let same = [128, 128, 128];
    assert!(contrast_ratio(lift_to_contrast(same, same, FLOOR), same) >= FLOOR);
    for value in 0u8..=255 {
        let ground = [value; 3];
        let lifted = lift_to_contrast([value; 3], ground, FLOOR);
        assert!(
            contrast_ratio(lifted, ground) >= FLOOR,
            "grey {value} failed"
        );
    }
}

/// `contrast.rs:pole` must select the more-contrasting pole on a mid-grey ground; otherwise a
/// familiar luminance-threshold simplification chooses white and drops every derived colour below
/// its floor.
#[test]
fn pole_picks_black_for_the_mid_grey_that_defeats_a_half_luminance_threshold() {
    let mid_grey = [128, 128, 128];

    assert!(relative_luminance(mid_grey) > 0.179);
    assert!(relative_luminance(mid_grey) < 0.5);
    assert_eq!(pole(mid_grey), [0, 0, 0]);
    assert!(contrast_ratio(pole(mid_grey), mid_grey) > contrast_ratio(WHITE, mid_grey));
}

/// `contrast.rs:contrast_ratio` must remain a symmetric WCAG ratio; otherwise the stated floors
/// vary with argument order and a contrast check can approve an unreadable colour.
#[test]
fn contrast_ratio_is_symmetric_and_has_known_endpoints() {
    let grey = [60, 120, 180];

    assert_eq!(contrast_ratio(grey, grey), 1.0);
    assert_eq!(contrast_ratio([0, 0, 0], WHITE), 21.0);
    assert_eq!(
        contrast_ratio(grey, [250, 250, 250]),
        contrast_ratio([250, 250, 250], grey)
    );
}

/// `contrast.rs:relative_luminance` must expand gamma; otherwise two greys straddling 3:1 against
/// black — #595959 at about 2.998 and #5a5a5a at about 3.045 — land on the same side of the floor.
#[test]
fn contrast_boundary_uses_linear_luminance() {
    assert!(contrast_ratio([89, 89, 89], [0, 0, 0]) < 3.0);
    assert!(contrast_ratio([90, 90, 90], [0, 0, 0]) >= 3.0);
    // Equal channel sums do not imply equal luminance: pure blue is below 3:1 on black.
    assert!(contrast_ratio([0, 0, 255], [0, 0, 0]) < 3.0);
}

/// `contrast.rs:mix` must preserve both endpoint colours; otherwise a full muting or full contrast
/// walk changes the target colour and misses the intended contrast floor.
#[test]
fn mix_is_exact_at_both_endpoints() {
    let from = [11, 22, 33];
    let to = [201, 202, 203];

    assert_eq!(mix(from, to, 0.0), from);
    assert_eq!(mix(from, to, 1.0), to);
}
