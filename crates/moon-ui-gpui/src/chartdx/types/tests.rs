use super::*;

/// Quote readouts must not alter base-quantity uploads or merge independent equal-time prints.
#[test]
fn trade_upload_preserves_base_quantity_for_quote_readouts() {
    let ticks = [
        moon_core::feed::Tick {
            time_ms: 1_010.0,
            price: 25.0,
            qty: 4.0,
            side: moon_core::feed::Side::Buy,
        },
        moon_core::feed::Tick {
            time_ms: 1_010.0,
            price: 50.0,
            qty: 4.0,
            side: moon_core::feed::Side::Sell,
        },
        moon_core::feed::Tick {
            time_ms: 1_011.0,
            price: f32::MAX,
            qty: 2.0,
            side: moon_core::feed::Side::Buy,
        },
    ];
    let mut uploaded = Vec::new();
    super::fill_cross_upload(&ticks, 1_000.0, &mut uploaded);
    assert_eq!(uploaded.len(), 3);
    assert_eq!(
        (uploaded[0].time_rel, uploaded[0].side, uploaded[0].qty),
        (10.0, 0, 4.0)
    );
    assert_eq!(
        (uploaded[1].time_rel, uploaded[1].side, uploaded[1].qty),
        (10.0, 1, 4.0)
    );
    assert_eq!(uploaded[2].qty, 2.0);
    let amounts: Vec<_> = uploaded
        .iter()
        .map(|c| moon_chart::tick_volume::quote_notional(c.price, c.qty))
        .collect();
    assert_eq!(
        amounts,
        [100.0, 200.0, 0.0],
        "unrepresentable quote amount must omit only its readout"
    );
}

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
