//! Unit coverage for the shot's final-size normalization rule.

use super::{
    ascent_fallback_px, fitted, lead_px, normalize, strip_height, RgbFrame, FONT_MAX_PX,
    FONT_MIN_PX, HEADER_RESERVE_PX, NORMALIZED_MAX_PX,
};

/// `resize.rs:normalize` must return a small capture byte-for-byte; otherwise the hotkey silently
/// upscales and blurs a chart that was already lossless at its user's chosen size.
#[test]
fn a_small_frame_stays_byte_for_byte_untouched() {
    let original = RgbFrame {
        width: 640,
        height: 480,
        rgb: (0..640 * 480 * 3)
            .map(|value| (value % 251) as u8)
            .collect(),
    };
    let normalized = normalize(RgbFrame {
        width: original.width,
        height: original.height,
        rgb: original.rgb.clone(),
    });

    assert_eq!(normalized.width, original.width);
    assert_eq!(normalized.height, original.height);
    assert_eq!(normalized.rgb, original.rgb);
}

/// `resize.rs:HEADER_RESERVE_PX` must reserve one header strip; otherwise a tall chart's composed
/// image misses the messenger limit or needlessly leaves an entire extra strip of resolution unused.
#[test]
fn a_tall_frame_leaves_room_for_one_burnt_in_header_strip() {
    let max_body_height = NORMALIZED_MAX_PX - HEADER_RESERVE_PX;
    assert_eq!(fitted(640, 480), None);
    assert_eq!(max_body_height, 1_225);
    assert_eq!(fitted(1_000, 2_000), Some((613, max_body_height)));

    let normalized = normalize(RgbFrame {
        width: 1_000,
        height: 2_000,
        rgb: vec![17; 1_000 * 2_000 * 3],
    });
    assert_eq!((normalized.width, normalized.height), (613, 1_225));
    assert_eq!(
        normalized.width.max(normalized.height + HEADER_RESERVE_PX),
        NORMALIZED_MAX_PX
    );
}

/// `resize.rs:strip_height` must include the lead font and hairline; otherwise the reserved box is
/// five pixels too short and the messenger recompresses a tall screenshot.
#[test]
fn the_maximum_strip_height_is_the_reserved_header_height() {
    assert_eq!(strip_height(FONT_MAX_PX), HEADER_RESERVE_PX);
}

/// `resize.rs:lead_px` must remain larger than its base font; otherwise the coin loses the visual
/// hierarchy that distinguishes a screenshot subject from its context.
#[test]
fn the_lead_font_is_larger_across_the_supported_base_range() {
    for base in FONT_MIN_PX..=FONT_MAX_PX {
        assert!(
            lead_px(base) > base,
            "base font {base} must have a larger lead"
        );
    }
}

/// `resize.rs:ascent_fallback_px` must stay inside the lead box; otherwise a failed GDI metrics
/// read positions the shared screenshot's baseline outside its own header strip.
#[test]
fn the_baseline_fallback_stays_inside_the_lead_font_box() {
    for base in FONT_MIN_PX..=FONT_MAX_PX {
        let lead = lead_px(base);
        assert!(ascent_fallback_px(lead) < lead);
    }
}

/// `resize.rs:strip_height` must be monotone in its base font; otherwise a wider chart can reserve
/// less room for a visibly larger header and overrun the messenger image limit.
#[test]
fn strip_height_never_shrinks_as_the_base_font_grows() {
    let heights: Vec<u32> = (FONT_MIN_PX..=FONT_MAX_PX).map(strip_height).collect();
    assert!(heights.windows(2).all(|pair| pair[0] <= pair[1]));
}

/// `resize.rs:fitted` must clamp an extreme ratio to a one-pixel side; otherwise PNG encoding
/// receives a zero-height image and a valid screenshot is lost.
#[test]
fn even_an_extreme_ratio_never_scales_to_zero_height() {
    assert_eq!(fitted(10_000_000, 1), Some((NORMALIZED_MAX_PX, 1)));
}
