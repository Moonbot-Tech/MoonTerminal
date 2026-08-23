//! Regression coverage for the RGB-to-DIB conversion used by chart shots.

use super::{DibImage, rgb_to_dib};

/// `win.rs:rgb_to_dib` and `DibImage::to_rgb_top_down` must preserve RGB pixels through BGR
/// conversion, bottom-up rows, and DWORD padding; otherwise a clipboard shot silently pastes with
/// swapped colors, inverted rows, or sheared scan lines.
#[test]
fn a_padded_three_pixel_frame_round_trips_back_to_its_original_rgb_bytes() {
    let rgb = vec![
        255, 0, 0, 0, 255, 0, 0, 0, 255, // top row
        12, 34, 56, 78, 90, 12, 34, 56, 78, // bottom row
    ];

    let dib: DibImage = rgb_to_dib(3, 2, &rgb).expect("a complete RGB frame converts to DIB");
    assert_eq!(
        dib.stride(),
        12,
        "three 24-bit pixels require DWORD padding"
    );
    assert_eq!(dib.to_rgb_top_down(), rgb);
}
