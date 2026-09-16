//! Regression cases for core-coloured arrival flashes and their contrast fallback.

use super::arrival_color;

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

/// Omitting or reversing the contrast check allows invisible dark-on-dark and light-on-light flashes.
#[test]
fn flash_falls_back_for_unreadable_or_missing_colors() {
    assert_eq!(
        arrival_color(Some([24, 24, 24]), [20, 20, 20], 0xff8800),
        0xff8800
    );
    assert_eq!(
        arrival_color(Some([250, 250, 250]), [255, 255, 255], 0xff8800),
        0xff8800
    );
    assert_eq!(arrival_color(None, [20, 20, 20], 0xff8800), 0xff8800);
    assert_eq!(arrival_color(None, [255, 255, 255], 0xff8800), 0xff8800);
}

/// Gamma-free channel averages misclassify these adjacent greys around 3:1 against black.
#[test]
fn flash_contrast_boundary_uses_linear_luminance() {
    // Independent sRGB values: #595959 is about 2.998:1, #5a5a5a about 3.045:1.
    assert_eq!(
        arrival_color(Some([89, 89, 89]), [0, 0, 0], 0xff8800),
        0xff8800
    );
    assert_eq!(
        arrival_color(Some([90, 90, 90]), [0, 0, 0], 0xff8800),
        0x5a5a5a
    );
    // Equal channel sums do not imply equal luminance: blue is below 3:1 on black.
    assert_eq!(
        arrival_color(Some([0, 0, 255]), [0, 0, 0], 0xff8800),
        0xff8800
    );
}
