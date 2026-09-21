//! Track-cap math for the shared settings slider row.
//!
//! Breakage: dropping the caption floor lets a long endpoint pair overflow the track; replacing
//! the height multiple with a window-sized number stretches every settings slider across a wide
//! pane.

// NOT `use super::*;`: the parent imports `gpui::*`, whose `test` macro shadows `#[test]`.
use super::{SLIDER_TRACK_HEIGHTS, slider_track_max_w};

/// Twelve 22 design-px slider heights is 264, the comfortable track beside the 260-wide valuation
/// select on the same General form. Captions wider than that floor win so the endpoints cannot
/// collide.
#[test]
fn slider_track_uses_caption_floor_or_twelve_heights() {
    assert_eq!(SLIDER_TRACK_HEIGHTS, 12.0);
    let slider_h = 22.0;
    assert_eq!(slider_track_max_w(40.0, slider_h), 264.0);
    assert_eq!(slider_track_max_w(264.0, slider_h), 264.0);
    assert_eq!(slider_track_max_w(272.0, slider_h), 272.0);
}
