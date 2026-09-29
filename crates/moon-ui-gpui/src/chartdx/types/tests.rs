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

/// `types.rs:queue_appended_ranges` must queue each append's wrapped runs for the incremental
/// bake and refuse once the queue would cover the ring. Queuing past that point would redraw an
/// overwritten slot twice, doubling a translucent tick in the Metal bitmap until the next rebake.
#[test]
fn queue_appended_ranges_splits_at_the_wrap_and_refuses_a_full_ring() {
    let mut pending = Vec::new();
    assert!(queue_appended_ranges(&mut pending, 8, 3, 10));
    assert_eq!(pending, vec![(8, 2), (0, 1)]);
    assert!(queue_appended_ranges(&mut pending, 1, 2, 10));
    assert_eq!(pending, vec![(8, 2), (0, 1), (1, 2)]);
    assert!(!queue_appended_ranges(&mut pending, 3, 5, 10));
    assert_eq!(pending, vec![(8, 2), (0, 1), (1, 2)]);
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

fn tick_at(time_rel: f32) -> ChartCross {
    ChartCross {
        time_rel,
        price: 100.0,
        side: 0,
        qty: 1.0,
    }
}

/// A full ring whose head sits mid-buffer: slots 3..6 hold times 0..3, slots 0..3 hold 3..6.
fn wrapped_ring() -> Vec<ChartCross> {
    [3.0, 4.0, 5.0, 0.0, 1.0, 2.0]
        .into_iter()
        .map(tick_at)
        .collect()
}

#[test]
fn span_runs_keep_only_the_span_in_ascending_slot_order() {
    let ring = wrapped_ring();
    // Times 2..=4 live in slot 5 (time 2) and slots 0..2 (times 3, 4); the runs start at slot 0.
    let runs = ring_span_runs(&ring, 3, 6, 6, 0.0, (2.0, 4.0));
    assert_eq!(runs, [(0, 2), (5, 1)]);
    let times: Vec<f32> = ring_run_slices(&ring, &runs)
        .into_iter()
        .flatten()
        .map(|c| c.time_rel)
        .collect();
    assert_eq!(times, [3.0, 4.0, 2.0]);
}

#[test]
fn span_runs_widen_by_lateness_evidence() {
    // Slot order 0, 5, 1: the row at time 1 arrives 4 late, so a search must not skip it.
    let ring: Vec<ChartCross> = [0.0, 5.0, 1.0].into_iter().map(tick_at).collect();
    let strict = ring_span_runs(&ring, 3, 3, 3, 0.0, (1.0, 1.0));
    let widened = ring_span_runs(&ring, 3, 3, 3, 4.0, (1.0, 1.0));
    let covered = |runs: [(usize, usize); 2]| {
        ring_run_slices(&ring, &runs)
            .into_iter()
            .flatten()
            .any(|c| c.time_rel == 1.0)
    };
    assert!(!covered(strict));
    assert!(covered(widened));
}

#[test]
fn run_slices_clamp_to_the_ring_and_drop_empty_runs() {
    let ring = wrapped_ring();
    let slices = ring_run_slices(&ring, &[(4, 10), (0, 0), (9, 2)]);
    assert_eq!(slices.len(), 1);
    assert_eq!(slices[0].len(), 2);
}

fn bake_columns(width_px: u32) -> moon_chart::tick_volume::BakeColumns {
    moon_chart::tick_volume::BakeColumns {
        time0: 0.0,
        time_to_px: 1.0,
        price0: 0.0,
        price_to_px: 1.0,
        height: 200.0,
        width_px,
        volume_alpha: 0.5,
        marker_half: 3.0,
        buy_inv: 1.0,
        sell_inv: 1.0,
    }
}

#[test]
fn lod_rows_leave_a_sparse_span_to_the_raw_runs() {
    let ring: Vec<ChartCross> = (0..4).map(|i| tick_at(i as f32)).collect();
    let mut pick = moon_chart::tick_volume::LodPick::default();
    let mut out = vec![tick_at(-1.0)];
    let thinned = lod_bake_rows(
        &ring,
        [(0, 4), (0, 0)],
        &bake_columns(100),
        true,
        &mut pick,
        &mut out,
    );
    assert!(!thinned);
    assert_eq!(
        out.len(),
        1,
        "a sparse span leaves the gathered rows untouched"
    );
}

#[test]
fn lod_rows_thin_a_dense_span_in_draw_order() {
    // 400 rows stacked on four texels of a 10-px bitmap: far past two rows per column.
    let ring: Vec<ChartCross> = (0..400).map(|i| tick_at((i % 4) as f32)).collect();
    let mut pick = moon_chart::tick_volume::LodPick::default();
    let mut out = Vec::new();
    let thinned = lod_bake_rows(
        &ring,
        [(0, 400), (0, 0)],
        &bake_columns(10),
        true,
        &mut pick,
        &mut out,
    );
    assert!(thinned);
    assert!(!out.is_empty() && out.len() < 400);
    let slots: Vec<u32> = pick.cross.clone();
    assert!(
        slots.windows(2).all(|w| w[0] < w[1]),
        "picked rows keep ring order"
    );
    // The last row of every texel survives, so the top-most colour is the one a raw draw ends on.
    for slot in 396..400u32 {
        assert!(slots.contains(&slot));
    }
}
