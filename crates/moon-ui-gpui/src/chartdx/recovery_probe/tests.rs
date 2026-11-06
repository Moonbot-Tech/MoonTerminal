//! Independent pixels and real D3D readback regressions for the native recovery oracle.

use super::{RecoveryImage, capture};
use crate::chartdx::gpu::test_support::TestTarget;

/// A lost single-pixel figure must fail even with unchanged dimensions, tick count and generation.
/// Replacing the exact comparison with a count/generation-only check reddens this assertion.
#[test]
fn recovery_image_rejects_one_missing_pixel_and_blank_baseline() {
    let mut before = RecoveryImage {
        generation: 1,
        width: 100,
        height: 50,
        pixels: vec![0; 100 * 50 * 4],
    };
    assert!(
        before.validate().is_err(),
        "equal blank images prove nothing"
    );
    for pixel in before.pixels[40..120].chunks_exact_mut(4) {
        pixel.copy_from_slice(&[0, 255, 0, 255]);
    }
    before.validate().unwrap();
    let mut after = RecoveryImage {
        generation: 2,
        width: 100,
        height: 50,
        pixels: before.pixels.clone(),
    };
    before.compare(&after).unwrap();
    after.pixels[40..44].fill(0);
    assert!(
        before.compare(&after).is_err(),
        "one lost figure pixel must not pass recovery"
    );
}

/// Read the actual native RTV and crop rather than hashing cached CPU state or padded GPU rows.
#[test]
fn recovery_readback_crops_actual_d3d_target_pixels() {
    let target = TestTarget::new(12, 8);
    let raw = target.raw(7);
    let (device, context, rtv) = crate::chartdx::gpu::borrow_d3d(&raw).unwrap();
    unsafe {
        context.ClearRenderTargetView(&rtv, &[1., 0., 0., 1.]);
    }
    let image = capture(&device, &context, &rtv, [3., 2., 8., 6.], 7).unwrap();
    assert_eq!((image.width, image.height, image.generation), (5, 4, 7));
    assert_eq!(image.pixels, [0, 0, 255, 255].repeat(20));
    unsafe {
        context.ClearRenderTargetView(&rtv, &[0., 1., 0., 1.]);
    }
    let changed = capture(&device, &context, &rtv, [3., 2., 8., 6.], 8).unwrap();
    assert!(
        image.compare(&changed).is_err(),
        "GPU output changed without any tick counter changing"
    );
}
