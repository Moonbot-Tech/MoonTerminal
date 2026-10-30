//! CPU regressions for patching hovered trade arrows in the userdata marker union.

use moon_chart::trade_marks::{
    self, CONNECTOR_THICKNESS, TRADE_HOVER_SCALE, TradeGeometryCtx, TradeMark,
};

use super::*;
use crate::chartdx::types::mk_of;

fn marks() -> Vec<TradeMark> {
    (0..5)
        .map(|i| TradeMark {
            buy_ms: 2_000_000 + i * 500_000,
            close_ms: 2_200_000 + i * 500_000,
            buy_price: 50.0 + i as f64,
            sell_price: 49.0 + i as f64,
            qty: 2.0,
            is_short: i % 2 == 0,
            show_entry: true,
            show_exit: true,
        })
        .collect()
}

fn ctx(hovered: Option<(usize, bool)>) -> TradeGeometryCtx {
    TradeGeometryCtx {
        epoch_ms: 1_000_000.0,
        long_rgb: [0, 180, 90],
        short_rgb: [200, 30, 30],
        scale: 1.5,
        px_per_ms: 0.002,
        px_per_price: 10.0,
        arrow_scale: 1.3,
        connector_thickness: CONNECTOR_THICKNESS,
        hovered,
    }
}

fn gpu(markers: &[MarkerInstance]) -> Vec<[u8; 48]> {
    markers.iter().map(|m| bytemuck::cast(mk_of(m))).collect()
}

/// `userdata.rs:UserDataLayer::patch_markers` writing only part of the instance (its position and
/// size, not its colour), or `trade_marks.rs:trade_marker` drifting from what
/// `build_trade_geometry` emits, leaves the hovered arrow drawn wrong until the next full rebuild.
/// After patching the old and new hot arrows, the marker union must be bitwise the union a full
/// rebuild with the new hover produces, and the hot arrow must be the resting one grown by
/// `TRADE_HOVER_SCALE` and fully opaque.
#[test]
fn patched_hover_equals_a_full_rebuild() {
    let marks = marks();
    let build = |hovered| {
        let (mut markers, mut segs) = (Vec::new(), Vec::new());
        let clusters =
            trade_marks::build_trade_geometry(&marks, &ctx(hovered), &mut markers, &mut segs);
        (clusters, markers)
    };
    let (clusters, cold) = build(None);
    let first = Some((1, true));
    let second = Some((4, false));
    let (_, hot_first) = build(first);
    let (_, hot_second) = build(second);

    let mut layer = UserDataLayer::new();
    let zone = [];
    layer.set(&zone, &[], &[], &cold);
    let at = |h: Option<(usize, bool)>| {
        clusters
            .iter()
            .position(|c| trade_marks::cluster_is_hot(c, h))
            .unwrap() as u32
    };
    take_marker_patches();
    let (i1, i2) = (at(first), at(second));
    assert!(layer.patch_markers(&[(
        i1,
        trade_marks::trade_marker(&clusters[i1 as usize], &ctx(None), true)
    )]));
    let pending = |layer: &UserDataLayer| -> Vec<[u8; 48]> {
        let p = layer.pending.as_ref().expect("pending union");
        p.mk.iter().map(|m| bytemuck::cast(*m)).collect()
    };
    assert_eq!(pending(&layer), gpu(&hot_first));
    assert!(layer.patch_markers(&[
        (
            i1,
            trade_marks::trade_marker(&clusters[i1 as usize], &ctx(None), false)
        ),
        (
            i2,
            trade_marks::trade_marker(&clusters[i2 as usize], &ctx(None), true)
        ),
    ]));
    assert_eq!(pending(&layer), gpu(&hot_second));
    assert_eq!(take_marker_patches(), 2);

    // Independent of the builder: grown by the hover scale, opaque, everything else the same.
    let (rest, hot) = (cold[i2 as usize], hot_second[i2 as usize]);
    assert_eq!(hot.size, rest.size * TRADE_HOVER_SCALE);
    assert_eq!(hot.thickness, rest.thickness * TRADE_HOVER_SCALE);
    assert_eq!(hot.color[3], 1.0);
    assert!(rest.color[3] < 1.0);
    assert_eq!(hot.color[..3], rest.color[..3]);
    assert_eq!(
        (hot.t_rel, hot.price, hot.shape),
        (rest.t_rel, rest.price, rest.shape)
    );

    // An index past the union is refused and changes nothing.
    assert!(!layer.patch_markers(&[(cold.len() as u32, hot)]));
    assert_eq!(pending(&layer), gpu(&hot_second));
}

/// Breakage: `UserDataLayer::set` keeps a hover patch when the caller hands the resting union
/// back. Consequence: the arrow stays enlarged after the geometry sync that replaced the patch.
#[test]
fn set_after_a_patch_restores_the_unpatched_markers() {
    let marks = marks();
    let (mut markers, mut segs) = (Vec::new(), Vec::new());
    let clusters = trade_marks::build_trade_geometry(&marks, &ctx(None), &mut markers, &mut segs);
    let mut layer = UserDataLayer::new();
    layer.set(&[], &[], &[], &markers);
    let hot = Some((1, true));
    let ix = clusters
        .iter()
        .position(|c| trade_marks::cluster_is_hot(c, hot))
        .unwrap() as u32;
    assert!(layer.patch_markers(&[(
        ix,
        trade_marks::trade_marker(&clusters[ix as usize], &ctx(None), true)
    )]));
    layer.set(&[], &[], &[], &markers);
    let pending = layer.pending.as_ref().expect("pending union");
    assert_eq!(
        pending
            .mk
            .iter()
            .map(|m| bytemuck::cast(*m))
            .collect::<Vec<[u8; 48]>>(),
        gpu(&markers)
    );
}

/// Breakage: a one-marker edit remaps every marker. Consequence: the CPU cost follows the whole
/// trade-arrow union instead of the edit.
#[test]
fn a_tail_edit_keeps_the_other_marker_bytes() {
    let marks = marks();
    let (mut markers, mut segs) = (Vec::new(), Vec::new());
    trade_marks::build_trade_geometry(&marks, &ctx(None), &mut markers, &mut segs);
    let mut layer = UserDataLayer::new();
    layer.set(&[], &[], &[], &markers);
    let before = layer.pending.as_ref().unwrap().mk.clone();
    let mut edited = markers.clone();
    edited[0].t_rel += 3.0;
    layer.set(&[], &[], &[], &edited);
    let after = &layer.pending.as_ref().unwrap().mk;
    assert_eq!(after.len(), before.len());
    assert_ne!(
        bytemuck::bytes_of(&after[0]),
        bytemuck::bytes_of(&before[0])
    );
    for (left, right) in after.iter().zip(before.iter()).skip(1) {
        assert_eq!(bytemuck::bytes_of(left), bytemuck::bytes_of(right));
    }
}

/// Scale probe: rebuilding every segment and marker versus the same call again.
/// Ignored: it allocates the unions and prints timings. The decision to skip unchanged
/// layers is taken from these medians.
#[test]
#[ignore = "scale probe: userdata set at 20k and 100k"]
fn userdata_set_scale_probe() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(userdata_set_scale_probe_body)
        .expect("userdata probe thread")
        .join()
        .expect("userdata probe thread");
}

fn userdata_set_scale_probe_body() {
    use bytemuck::Zeroable;
    use moon_chart::layers::{MarkerInstance, SegInstance};

    fn median(mut samples: Vec<f64>) -> f64 {
        samples.sort_by(|a, b| a.total_cmp(b));
        samples[samples.len() / 2]
    }

    fn time_n(n: usize) -> (f64, f64) {
        const RUNS: usize = 7;
        let segs: Vec<SegInstance> = (0..n)
            .map(|i| {
                let mut seg = SegInstance::zeroed();
                seg.t0_rel = i as f32;
                seg
            })
            .collect();
        let markers: Vec<MarkerInstance> = (0..n)
            .map(|i| {
                let mut marker = MarkerInstance::zeroed();
                marker.t_rel = i as f32;
                marker
            })
            .collect();
        let mut layer = UserDataLayer::new();
        layer.set(&[], &[], &segs, &markers);
        let mut mutate_ms = Vec::with_capacity(RUNS);
        let mut repeat_ms = Vec::with_capacity(RUNS);
        for i in 0..RUNS {
            let mut changed = segs.clone();
            changed[n - 1].p0 = 1.0 + i as f32;
            let start = std::time::Instant::now();
            layer.set(&[], &[], &changed, &markers);
            std::hint::black_box(layer.pending.as_ref().map(|p| p.seg.len() + p.mk.len()));
            mutate_ms.push(start.elapsed().as_secs_f64() * 1_000.0);

            let start = std::time::Instant::now();
            layer.set(&[], &[], &changed, &markers);
            std::hint::black_box(layer.pending.as_ref().map(|p| p.seg.len() + p.mk.len()));
            repeat_ms.push(start.elapsed().as_secs_f64() * 1_000.0);
        }
        (median(mutate_ms), median(repeat_ms))
    }

    for n in [20_000, 100_000] {
        let (mutate_ms, repeat_ms) = time_n(n);
        println!("userdata_set_scale n {n} mutate_ms {mutate_ms:.3} repeat_ms {repeat_ms:.3}");
    }
}
