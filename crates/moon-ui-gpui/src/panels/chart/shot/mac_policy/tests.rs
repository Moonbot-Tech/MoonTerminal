//! Regressions for caption expiry during asynchronous reads and window-base coordinates.

use super::{crop_pixels, publishable, source_rect};
use std::time::{Duration, Instant};

/// Removing the completion-time checks would publish after a timeout or a second hotkey press.
#[test]
fn rejects_expired_replaced_and_undrawn_captures() {
    let now = Instant::now();
    let deadline = now + Duration::from_secs(1);
    assert!(publishable(7, 7, true, deadline, now));
    assert!(!publishable(7, 8, true, deadline, now));
    assert!(!publishable(7, 7, false, deadline, now));
    assert!(!publishable(7, 7, true, deadline, deadline));
    assert!(!publishable(
        7,
        7,
        true,
        deadline,
        deadline + Duration::from_nanos(1)
    ));
}

/// Omitting the top-left flip or titlebar offset copies chrome instead of the chart slot.
#[test]
fn flips_window_base_and_refuses_partial_slots() {
    assert_eq!(
        source_rect([10.0, 20.0, 500.0, 300.0], [800.0, 622.0]),
        Some([10.0, 302.0, 500.0, 300.0])
    );
    assert_eq!(
        source_rect([0.0, 0.0, 800.0, 622.0], [800.0, 622.0]),
        Some([0.0, 0.0, 800.0, 622.0])
    );
    assert_eq!(source_rect([0.0, 0.0, 801.0, 622.0], [800.0, 622.0]), None);
    assert_eq!(source_rect([-1.0, 0.0, 500.0, 300.0], [800.0, 622.0]), None);
    assert_eq!(
        source_rect([0.0, f64::NAN, 500.0, 300.0], [800.0, 622.0]),
        None
    );
}

/// Capturing the full Retina window must still copy only the slot, never resize chrome into it.
#[test]
fn crops_retina_pixels_and_rejects_truncated_window_images() {
    assert_eq!(
        crop_pixels([0.0, -1.0, 500.0, 300.0], 1.0, [800, 600]),
        None
    );
    assert_eq!(
        crop_pixels([0.0, 301.0, 500.0, 300.0], 1.0, [800, 600]),
        None
    );
    assert_eq!(
        crop_pixels([10.0, 302.0, 500.0, 300.0], 2.0, [1600, 1244]),
        Some([20, 604, 1000, 600])
    );
    assert_eq!(
        crop_pixels([10.0, 302.0, 500.0, 300.0], 1.0, [800, 622]),
        Some([10, 302, 500, 300])
    );
    assert_eq!(
        crop_pixels([10.0, 302.0, 500.0, 300.0], 2.0, [1000, 600]),
        None
    );
    assert_eq!(
        crop_pixels([0.0, 0.0, 800.0, 622.0], 2.0, [1600, 1244]),
        Some([0, 0, 1600, 1244])
    );
    assert_eq!(
        crop_pixels([0.0, 0.0, 801.0, 622.0], 2.0, [1600, 1244]),
        None
    );
}
