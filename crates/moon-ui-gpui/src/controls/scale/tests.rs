use super::{SCALES, normalized_scale};

/// Relaxing `scale.rs:normalized_scale` to accept any finite positive value must fail: a hand-edited
/// stored scale changes the chart while its dropdown falsely says Auto after restart.
#[test]
fn persisted_scales_are_limited_to_the_dropdowns_actual_presets() {
    for (_, preset) in SCALES {
        assert_eq!(normalized_scale(preset), preset);
    }
    for invalid in [0.49, -0.5, 0.0, f32::NAN, f32::INFINITY] {
        assert_eq!(
            normalized_scale(Some(invalid)),
            None,
            "invalid stored value {invalid:?}"
        );
    }
}
