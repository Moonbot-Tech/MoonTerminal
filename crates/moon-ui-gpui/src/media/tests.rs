//! The channel-order contract every asset loader now goes through.

use super::{render_image_bgra, render_image_from_png};

/// GPUI draws these images as BGRA; a PNG decodes to RGBA. The swap is the whole reason this helper
/// exists, and it is invisible when wrong — the image still draws, just with red and blue traded.
///
/// Breakage: dropping the swap turns every coin icon, group icon and exchange logo blue-for-red at
/// once, which no compiler and no layout test can see. The oracle is a hand-built PNG whose one
/// pixel is unambiguous: pure red in, red in the RED slot of the BGRA buffer out.
#[test]
fn a_decoded_png_reaches_gpui_in_bgra_order() {
    let mut png = Vec::new();
    let red = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
    image::DynamicImage::ImageRgba8(red)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("the fixture must encode");

    let image = render_image_from_png(&png).expect("a valid PNG must decode");
    let pixel = image.as_bytes(0).expect("one frame");
    assert_eq!(
        (pixel[0], pixel[1], pixel[2], pixel[3]),
        (0, 0, 255, 255),
        "pure red must arrive as B=0 G=0 R=255, not as RGBA"
    );
}

/// A buffer that does not match its dimensions must be refused rather than drawn.
///
/// Breakage: constructing the image anyway hands GPUI a short buffer, which is a garbled texture at
/// best. Callers already treat `None` as "this asset has no icon", which is the honest outcome.
#[test]
fn a_mismatched_buffer_yields_no_image() {
    assert!(render_image_bgra(2, 2, vec![0; 4]).is_none());
    assert!(render_image_bgra(1, 1, vec![1, 2, 3, 4]).is_some());
}

/// `render_image_bgra` must pass its bytes through untouched.
///
/// It is the single entry point every loader now ends on, and one of them — the SVG rasterizer —
/// already hands over BGRA. Breakage: moving the RGBA→BGRA swap down into this helper looks like a
/// tidy-up and leaves the PNG test green, while double-swapping every exchange logo back to RGBA.
#[test]
fn the_shared_wrapper_does_not_touch_the_bytes_it_is_given() {
    let bgra = vec![10, 20, 30, 40];
    let image = render_image_bgra(1, 1, bgra.clone()).expect("a matching buffer must wrap");
    assert_eq!(
        image.as_bytes(0).expect("one frame"),
        bgra.as_slice(),
        "the wrapper must not reorder channels; only the PNG path converts"
    );
}
