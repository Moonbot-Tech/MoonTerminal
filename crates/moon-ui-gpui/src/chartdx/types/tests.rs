use super::*;

#[test]
fn evicted_cross_ranges_reports_overwritten_ring_slots() {
    assert_eq!(cross_append_ranges(3, 4, 5), [(3, 2), (0, 2)]);
    assert_eq!(evicted_cross_ranges(0, 3, 5, 3), [(0, 1), (0, 0)]);
    assert_eq!(evicted_cross_ranges(2, 5, 5, 2), [(2, 2), (0, 0)]);
    assert!(ranges_have_entries(&evicted_cross_ranges(2, 5, 5, 2)));
    assert!(!ranges_have_entries(&evicted_cross_ranges(0, 2, 5, 2)));
}

#[test]
fn evicted_cross_ranges_handles_wrapped_full_ring() {
    assert_eq!(evicted_cross_ranges(4, 5, 5, 3), [(4, 1), (0, 2)]);
}

/// `types.rs:seg_of` must carry the pin flag into the slot the three seg shaders read it from.
///
/// The flag is the whole feature: dropped here, an exit line that left the price band is silently
/// clipped away again, and nothing fails to compile — the slot used to be a hard-coded zero. The
/// GPU struct stays three `float4`s wide, so no backend's stride moves with it.
#[test]
fn seg_of_carries_the_pin_flag_in_the_spare_slot() {
    let seg = moon_chart::layers::SegInstance {
        t0_rel: 1.0,
        p0: 2.0,
        t1_rel: 3.0,
        p1: 4.0,
        thickness: 5.0,
        pattern: 6.0,
        extend: moon_chart::layers::SEG_EXTEND_EDGE,
        clamp: moon_chart::layers::SEG_CLAMP_PLOT,
        color: [0.1, 0.2, 0.3, 0.4],
    };
    let gpu = seg_of(&seg);
    assert_eq!(gpu.m, [5.0, 6.0, 1.0, 1.0]);
    assert_eq!(
        std::mem::size_of::<SegGpu>(),
        48,
        "the pin flag must ride a spare slot, not widen the instance"
    );

    let plain = moon_chart::layers::SegInstance {
        clamp: moon_chart::layers::SEG_CLAMP_NONE,
        ..seg
    };
    assert_eq!(seg_of(&plain).m[3], 0.0);
}
