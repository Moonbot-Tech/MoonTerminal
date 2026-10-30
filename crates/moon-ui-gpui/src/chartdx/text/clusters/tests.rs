//! Bubble labels stay the set a per-frame walk would print, and a quiet frame does not walk again.

use moon_chart::clusters::{PrintDisc, label_fits, short_volume};

use super::{
    CLUSTER_LABEL_MAX, MONO_ADVANCE, ensure_cluster_label_plan, note_cluster_discs_published,
    take_cluster_label_scans,
};
use crate::chartdx::pane_render::PaneRender;
use crate::chartdx::types::ChartViewGpu;

fn wide<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(body)
        .expect("label test thread")
        .join()
        .expect("label test thread")
}

fn view_at(time0: f32) -> ChartViewGpu {
    ChartViewGpu {
        bounds: [10.0, 20.0, 800.0, 600.0],
        time_to_px: 1.0,
        price_to_px: 1.0,
        view_time0: time0,
        view_price0: 0.0,
        ..ChartViewGpu::default()
    }
}

fn disc(time_ms: f64, price: f32, total: f32, radius: f32, small: bool, key: i64) -> PrintDisc {
    PrintDisc {
        time_ms,
        price,
        key,
        buy: total,
        sell: 0.0,
        radius,
        small,
        first_ms: time_ms,
    }
}

/// The walk `draw_cluster_labels` used to do on every text frame, kept here as the oracle.
fn reference(
    discs: &[PrintDisc],
    view: ChartViewGpu,
    epoch_ms: f64,
    sf: f32,
    font_px: f32,
) -> Vec<(i64, f32, f32, f32)> {
    let line_h = font_px + 2.0;
    let glyph_w = font_px * MONO_ADVANCE;
    let (left, top) = (view.bounds[0], view.bounds[1]);
    let (right, bottom) = (left + view.bounds[2], top + view.bounds[3]);
    let mut found = Vec::new();
    for d in discs.iter().filter(|d| !d.small) {
        let t_rel = (d.time_ms - epoch_ms) as f32;
        let x = (left + (t_rel - view.view_time0) * view.time_to_px).round();
        let y = (bottom - (d.price - view.view_price0) * view.price_to_px).round();
        if x < left || x > right || y < top || y > bottom {
            continue;
        }
        let key = (d.first_ms as i64).wrapping_mul(31).wrapping_add(d.key);
        found.push((key, x / sf, y / sf, d.total(), d.radius / sf));
    }
    found.sort_unstable_by(|a, b| b.3.total_cmp(&a.3));
    let mut placed = Vec::new();
    let mut plan = Vec::new();
    for &(key, x, y, total, r) in &found {
        if plan.len() >= CLUSTER_LABEL_MAX {
            break;
        }
        let text = short_volume(total);
        let w = text.chars().count() as f32 * glyph_w;
        if !label_fits(r, w, line_h) {
            continue;
        }
        let rect = [x - w * 0.5, y - line_h * 0.5, x + w * 0.5, y + line_h * 0.5];
        let overlaps = placed.iter().any(|p: &[f32; 4]| {
            rect[0] < p[2] && rect[2] > p[0] && rect[1] < p[3] && rect[3] > p[1]
        });
        if overlaps {
            continue;
        }
        placed.push(rect);
        plan.push((key, x, y, total));
    }
    plan
}

fn bits(plan: &[(i64, f32, f32, f32)]) -> Vec<(i64, u32, u32, u32)> {
    plan.iter()
        .map(|(k, x, y, t)| (*k, x.to_bits(), y.to_bits(), t.to_bits()))
        .collect()
}

fn sample_discs() -> Vec<PrintDisc> {
    vec![
        disc(100.0, 100.0, 5_000.0, 40.0, false, 1),
        disc(100.0, 100.0, 1_000.0, 40.0, false, 2),
        disc(200.0, 200.0, 9_000.0, 40.0, true, 3),
        disc(5_000.0, 100.0, 8_000.0, 40.0, false, 4),
        disc(300.0, 150.0, 3_000.0, 40.0, false, 5),
        disc(400.0, 250.0, 2_500.0, 40.0, false, 6),
        disc(150.0, 400.0, 4_000.0, 2.0, false, 7),
        disc(500.0, 350.0, 1_500.0, 40.0, false, 8),
    ]
}

/// Breakage: the cached plan drops a disc the old per-frame walk would label, or labels one it
/// would reject (off-screen, small, too tight, or overlapping a bigger disc).
/// Consequence: bubble volumes move or vanish while the camera is still.
#[test]
fn cluster_label_plan_matches_a_per_frame_walk() {
    wide(cluster_label_plan_matches_a_per_frame_walk_body);
}

fn cluster_label_plan_matches_a_per_frame_walk_body() {
    let discs = sample_discs();
    let mut pane = PaneRender::new();
    pane.epoch_ms = 0.0;
    pane.view = view_at(0.0);
    pane.cluster_discs = discs.clone();
    let sf = 1.0;
    let font = 12.0;
    let _ = take_cluster_label_scans();
    ensure_cluster_label_plan(&mut pane, sf, font);
    let expected = reference(&discs, pane.view, pane.epoch_ms, sf, font);
    assert_eq!(bits(&pane.cluster_label_plan), bits(&expected));
    assert_eq!(
        pane.cluster_label_plan
            .iter()
            .map(|label| label.0)
            .collect::<Vec<_>>(),
        vec![
            (100_i64).wrapping_mul(31).wrapping_add(1),
            (300_i64).wrapping_mul(31).wrapping_add(5),
            (400_i64).wrapping_mul(31).wrapping_add(6),
            (500_i64).wrapping_mul(31).wrapping_add(8),
        ]
    );
    assert_eq!(take_cluster_label_scans(), 1);

    ensure_cluster_label_plan(&mut pane, sf, font);
    assert_eq!(take_cluster_label_scans(), 0);
    assert_eq!(bits(&pane.cluster_label_plan), bits(&expected));

    pane.view = view_at(25.0);
    ensure_cluster_label_plan(&mut pane, sf, font);
    let moved = reference(&discs, pane.view, pane.epoch_ms, sf, font);
    assert_eq!(bits(&pane.cluster_label_plan), bits(&moved));
    assert_ne!(bits(&moved), bits(&expected));
    assert_eq!(take_cluster_label_scans(), 1);

    note_cluster_discs_published(&mut pane);
    ensure_cluster_label_plan(&mut pane, sf, font);
    assert_eq!(take_cluster_label_scans(), 1);
    assert_eq!(
        bits(&pane.cluster_label_plan),
        bits(&reference(&discs, pane.view, pane.epoch_ms, sf, font))
    );
}

/// Scale probe: planning 100_000 visible discs versus drawing the plan again.
/// Ignored: it allocates the tape and prints timings.
#[test]
#[ignore = "scale probe: cluster label plan vs cache hit"]
fn cluster_label_scale_probe() {
    wide(cluster_label_scale_probe_body);
}

fn cluster_label_scale_probe_body() {
    const N: usize = 100_000;
    const RUNS: usize = 7;
    let discs: Vec<PrintDisc> = (0..N)
        .map(|i| {
            disc(
                30.0 + (i % 700) as f64,
                30.0 + (i % 500) as f32,
                (N - i) as f32,
                28.0,
                false,
                i as i64,
            )
        })
        .collect();
    let mut pane = PaneRender::new();
    pane.view = view_at(0.0);
    pane.cluster_discs = discs;
    let mut plan_ms = Vec::with_capacity(RUNS);
    let mut hit_ms = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        pane.cluster_label_key = None;
        let start = std::time::Instant::now();
        ensure_cluster_label_plan(&mut pane, 1.0, 12.0);
        std::hint::black_box(pane.cluster_label_plan.len());
        plan_ms.push(start.elapsed().as_secs_f64() * 1_000.0);

        let start = std::time::Instant::now();
        ensure_cluster_label_plan(&mut pane, 1.0, 12.0);
        std::hint::black_box(pane.cluster_label_plan.len());
        hit_ms.push(start.elapsed().as_secs_f64() * 1_000.0);
    }
    plan_ms.sort_by(|a, b| a.total_cmp(b));
    hit_ms.sort_by(|a, b| a.total_cmp(b));
    println!(
        "cluster_label_scale plan_ms {:.3} cache_hit_ms {:.3}",
        plan_ms[RUNS / 2],
        hit_ms[RUNS / 2]
    );
}
