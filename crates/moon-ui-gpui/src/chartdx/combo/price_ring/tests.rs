use super::*;

fn pts(times: std::ops::Range<usize>) -> Vec<PriceLinePoint> {
    times
        .map(|t| PriceLinePoint {
            time_rel_ms: t as f32,
            price: 100.0 + t as f32,
        })
        .collect()
}

/// Breakage: `combo/price_ring.rs` `PriceRing::physical` drops its `% cap` (or offsets from the
/// wrong slot). Consequence: price lines draw garbage segments across the ring wrap.
#[test]
fn wrapped_ring_reads_back_in_time_order_through_the_shader_offset() {
    let mut r = PriceRing::new(8);
    r.reset(&pts(0..5));
    r.append(&pts(5..11));
    assert_eq!(r.count, 8);
    // The newest 8 of the 11 pushed points, oldest first.
    let want: Vec<f32> = (3..11).map(|t| t as f32).collect();
    let got: Vec<Option<f32>> = (0..r.count)
        .map(|i| r.slots.get(r.physical(i)).map(|p| p.time_rel_ms))
        .collect();
    assert_eq!(got, want.iter().copied().map(Some).collect::<Vec<_>>());

    // Points 5..=8 intersect [5.5, 7.5] with one neighbour each side.
    let (lo, hi) = r.visible(5.5, 7.5);
    assert_eq!((lo, hi), (2, 6));
    assert_eq!(r.visible(-100.0, 1000.0), (0, 8));
    // The shader reads slot (offset + iid) % cap with offset = physical(lo).
    let offset = r.physical(lo);
    let via_shader: Vec<Option<f32>> = (0..hi - lo)
        .map(|iid| r.slots.get((offset + iid) % 8).map(|p| p.time_rel_ms))
        .collect();
    assert_eq!(
        via_shader,
        want[lo..hi].iter().copied().map(Some).collect::<Vec<_>>()
    );
}

/// m4 keeps each column's extremes and the endpoints within four points per column.
#[test]
fn m4_keeps_every_column_extreme_in_at_most_four_points_per_column() {
    let n = 10_000usize;
    let data: Vec<PriceLinePoint> = (0..n)
        .map(|i| PriceLinePoint {
            time_rel_ms: i as f32,
            price: ((i * 7919) % 1000) as f32,
        })
        .collect();
    let mut r = PriceRing::new(n);
    r.reset(&data);
    let mut out = Vec::new();
    r.m4(0.01, &mut out);
    assert!(out.len() <= 400, "{} points for 100 columns", out.len());
    let key = |p: &PriceLinePoint| (p.time_rel_ms, p.price);
    assert_eq!(out.first().map(key), data.first().map(key));
    assert_eq!(out.last().map(key), data.last().map(key));
    for col in data.chunks(100) {
        let lo = col.iter().map(|p| p.price).fold(f32::INFINITY, f32::min);
        let hi = col
            .iter()
            .map(|p| p.price)
            .fold(f32::NEG_INFINITY, f32::max);
        let t0 = col[0].time_rel_ms;
        let t1 = col[99].time_rel_ms;
        let inside = |p: &&PriceLinePoint| p.time_rel_ms >= t0 && p.time_rel_ms <= t1;
        assert!(out.iter().filter(inside).any(|p| p.price == lo));
        assert!(out.iter().filter(inside).any(|p| p.price == hi));
    }
}
