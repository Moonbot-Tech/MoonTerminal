//! Regression cases for core-coloured arrival flashes and their readability lift.

use super::arrival_color;
use crate::contrast::contrast_ratio;
use crate::design::u32_to_rgb;

/// Always returning accent loses core identity on both light and dark backgrounds.
#[test]
fn flash_preserves_readable_core_colors() {
    assert_eq!(
        arrival_color(Some([0, 255, 0]), [0, 0, 0], 0xff8800),
        0x00ff00
    );
    assert_eq!(
        arrival_color(Some([0, 0, 255]), [255, 255, 255], 0xff8800),
        0x0000ff
    );
}

/// Swapping an unreadable colour for the accent painted every pale core on the light theme the
/// same blue; the flash must instead keep the hue and reach the floor.
#[test]
fn flash_lifts_unreadable_colors_instead_of_swapping_them_for_accent() {
    for (core, background) in [
        ([24, 24, 24], [20, 20, 20]),
        ([250, 250, 250], [255, 255, 255]),
        ([0xF3, 0xBA, 0x2F], [255, 255, 255]),
        ([0x20, 0x20, 0x40], [30, 30, 30]),
    ] {
        let flashed = u32_to_rgb(arrival_color(Some(core), background, 0xff8800));
        assert_ne!(flashed, [0xff, 0x88, 0x00], "{core:?} fell back to accent");
        assert!(
            contrast_ratio(flashed, background) >= 3.0,
            "{core:?} -> {flashed:?}"
        );
    }
    // The yellow stays a yellow: red and green together, blue well behind.
    let yellow = u32_to_rgb(arrival_color(
        Some([0xF3, 0xBA, 0x2F]),
        [255, 255, 255],
        0xff8800,
    ));
    assert!(yellow[0] > yellow[2] + 60 && yellow[1] > yellow[2] + 60);
}

/// Only a core with no colour at all takes the accent, on either theme.
#[test]
fn flash_falls_back_to_accent_only_for_a_missing_color() {
    assert_eq!(arrival_color(None, [20, 20, 20], 0xff8800), 0xff8800);
    assert_eq!(arrival_color(None, [255, 255, 255], 0xff8800), 0xff8800);
}

/// Gamma-free channel averages misclassify these adjacent greys around 3:1 against black: the one
/// just under the floor must move — lifted, not swapped for the accent — and the one just over
/// must not.
#[test]
fn flash_contrast_boundary_uses_linear_luminance() {
    // Independent sRGB values: #595959 is about 2.998:1, #5a5a5a about 3.045:1.
    let under = arrival_color(Some([89, 89, 89]), [0, 0, 0], 0xff8800);
    assert_ne!(under, 0x595959);
    assert_ne!(under, 0xff8800, "fell back to accent");
    assert!(contrast_ratio(u32_to_rgb(under), [0, 0, 0]) >= 3.0);
    assert_eq!(
        arrival_color(Some([90, 90, 90]), [0, 0, 0], 0xff8800),
        0x5a5a5a
    );
    // Equal channel sums do not imply equal luminance: blue is below 3:1 on black and is lifted.
    let blue = arrival_color(Some([0, 0, 255]), [0, 0, 0], 0xff8800);
    assert_ne!(blue, 0x0000ff);
    assert_ne!(blue, 0xff8800, "fell back to accent");
    assert!(contrast_ratio(u32_to_rgb(blue), [0, 0, 0]) >= 3.0);
}
