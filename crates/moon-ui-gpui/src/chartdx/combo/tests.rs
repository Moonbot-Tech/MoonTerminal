//! CPU-only regression checks for native tick selection and bake-window scaling.

use super::{ChartCross, ComboLayer, TickTimeOrder};
use crate::chartdx::types::reset_cross_ring;

/// Reinstating overlap/wrap guards rebakes every dense live append after offscreen eviction.
#[test]
fn offscreen_eviction_with_overlapping_append_keeps_incremental_bake() {
    for head in [0, 3] {
        let mut layer = ComboLayer::new();
        layer.set_capacity(4, 1);
        let chronological = [
            cross(0.0, 0, 8.0),
            cross(1.0, 0, 8.0),
            cross(80.0, 0, 8.0),
            cross(100.0, 1, 8.0),
        ];
        let mut slots = chronological;
        slots.rotate_right(head);
        resident(&mut layer, &slots);
        layer.resident_head = head;
        layer.append(&[cross(100.0, 0, 1.0), cross(100.0, 1, 1.0)]);
        let span = (50.0, 250.0);
        assert!(!layer.append_invalidates_bake(layer.pending_append.len(), span));
        assert!(
            layer.append_invalidates_bake(3, span),
            "visible eviction must repaint"
        );
        assert!(
            layer.append_invalidates_bake(4, span),
            "full replacement must repaint"
        );
    }
}

/// Build distinct tick prices so selection assertions detect dropped or substituted rows.
fn cross(time: f32, side: u32, qty: f32) -> ChartCross {
    ChartCross {
        time_rel: time,
        side,
        qty,
        price: time + 100.0,
    }
}

/// Model an already-uploaded reset without creating a GPU device.
fn resident(layer: &mut ComboLayer, rows: &[ChartCross]) {
    reset_cross_ring(
        &mut layer.resident_crosses,
        &mut layer.resident_head,
        &mut layer.resident_count,
        layer.cross_capacity as usize,
        rows,
    );
    layer.tick_time_order = TickTimeOrder::default();
    layer
        .tick_time_order
        .extend(rows.iter().map(|c| c.time_rel));
}

/// Searching resident storage alone would expose evicted ticks and miss pending equal-time prints.
#[test]
fn bounded_samples_include_pending_eviction_and_reset() {
    let mut layer = ComboLayer::new();
    layer.set_capacity(4, 1);
    resident(
        &mut layer,
        &[
            cross(1.0, 0, 1.0),
            cross(2.0, 0, 2.0),
            cross(3.0, 1, 3.0),
            cross(4.0, 0, 4.0),
        ],
    );
    layer.append(&[cross(4.0, 1, 5.0), cross(5.0, 0, 6.0)]);
    assert_eq!(
        layer
            .tick_samples(2.0, 4.0)
            .map(|c| c.qty)
            .collect::<Vec<_>>(),
        [3.0, 4.0, 5.0]
    );
    layer.reset(vec![cross(20.0, 0, 7.0), cross(21.0, 1, 8.0)]);
    layer.append(&[cross(21.0, 0, 9.0)]);
    assert_eq!(
        layer
            .tick_samples(21.0, 21.0)
            .map(|c| c.qty)
            .collect::<Vec<_>>(),
        [8.0, 9.0]
    );
}

/// A sorted assumption or clearing evidence on resource loss drops out-of-order liquidation ticks.
#[test]
fn pending_disorder_survives_resource_recreation() {
    let mut layer = ComboLayer::new();
    layer.set_capacity(8, 1);
    layer.reset(vec![cross(10.0, 0, 1.0), cross(30.0, 1, 2.0)]);
    layer.append(&[cross(20.0, 2, 3.0)]);
    layer.refresh_pending_time_order();
    let selected: Vec<_> = layer
        .tick_samples(19.0, 21.0)
        .filter(|c| c.time_rel >= 19.0 && c.time_rel <= 21.0)
        .map(|c| c.qty)
        .collect();
    assert_eq!(selected, [3.0]);
}

/// Using the global maximum or dropping the +/-2px band edges changes visible bar heights.
#[test]
fn bounded_volume_scale_preserves_exact_band_window() {
    let mut layer = ComboLayer::new();
    layer.set_capacity(8, 1);
    resident(
        &mut layer,
        &[
            cross(0.0, 0, 999.0),
            cross(9.0, 0, 7.0),
            cross(10.0, 1, 4.0),
            cross(21.0, 1, 8.0),
            cross(22.0, 0, 100.0),
        ],
    );
    assert_eq!(
        layer.volume_scale_for_bake_window(10.0, 20.0, 2.0),
        (7.0, 8.0)
    );
    assert_eq!(
        layer.volume_scale_for_bake_window(11.0, 16.0, 2.0),
        (1e-6, 4.0)
    );
}

/// Clearing evidence on a capacity-sized append violates the conservative arrival-time bound.
#[test]
fn capacity_sized_append_keeps_lateness_until_reset() {
    let mut layer = ComboLayer::new();
    layer.set_capacity(2, 1);
    layer.reset(vec![cross(30.0, 0, 1.0), cross(10.0, 2, 1.0)]);
    layer.append(&[cross(40.0, 0, 1.0), cross(50.0, 1, 1.0)]);
    assert_eq!(layer.tick_time_order.max_lateness(), 20.0);
    layer.reset(vec![cross(60.0, 0, 1.0), cross(60.0, 1, 1.0)]);
    assert_eq!(layer.tick_time_order.max_lateness(), 0.0);
}

/// A live chart pane: 1000x600 px following a last price inside a +-50 trade window.
fn follow_pane() -> (moon_chart::view::ChartView, moon_chart::view::Rect, f64) {
    let epoch = 1.7e12;
    let mut view = moon_chart::view::ChartView::new(epoch);
    let area = moon_chart::view::Rect {
        x: 0.0,
        y: 0.0,
        w: 1000.0,
        h: 600.0,
    };
    view.update_y(epoch + 16.0, area.h, Some((50.0, 150.0)), Some(100.0));
    (view, area, epoch + 16.0)
}

fn gpu_of(
    view: &moon_chart::view::ChartView,
    area: moon_chart::view::Rect,
) -> crate::chartdx::gpu::ChartViewGpu {
    crate::chartdx::view::view_gpu(
        view,
        area,
        [area.w, area.h],
        1.0,
        crate::chartdx::view::ViewStyle::default(),
    )
}

/// Record a full cross bake into the key exactly as `ComboLayer` does after drawing it.
fn commit_cross(
    key: &mut super::plan::ComboBakeKey,
    plan: super::plan::CrossBakePlan,
    g: &crate::chartdx::gpu::ChartViewGpu,
) {
    key.bake_t0 = plan.bake_t0;
    key.bake_p0 = plan.bake_p0;
    key.time_to_px = g.time_to_px;
    key.price_to_px = g.price_to_px;
    key.marker_half = g.marker_half;
    key.valid = true;
}

/// `combo/plan.rs:plan_cross_bake` / `plan_volume_bake`: re-adding the view's price origin to
/// the rebake key (or letting Y into the volume plan) makes the combo bitmaps fully rebake on
/// every live price-axis sync again, the whole cost this goal removed; the blit must instead
/// slide the baked texture by the view's pixel motion.
#[test]
fn live_follow_inside_the_margin_never_rebakes_and_slides_the_blit() {
    use super::plan::{
        ComboBakeKey, VolumeBakeKey, combo_v_margin_px, combo_x_margin_px, cross_blit_uv,
        plan_cross_bake, plan_volume_bake,
    };
    let (mut view, area, mut now) = follow_pane();
    let (bw, bh) = (area.w, area.h);
    let v_margin = combo_v_margin_px(bh);
    let tex_w = (bw + combo_x_margin_px(bw)).round() as u32;
    let tex_h_total = bh as u32 + 2 * v_margin as u32;
    let mut key = ComboBakeKey::unbaked(tex_w, tex_h_total, v_margin);
    let mut vkey = VolumeBakeKey::unbaked(tex_w, 72, bh as u32);
    let g0 = gpu_of(&view, area);
    let first = plan_cross_bake(&key, &g0, bw);
    assert!(first.full, "an unbaked texture bakes");
    commit_cross(&mut key, first, &g0);
    let vfirst = plan_volume_bake(&vkey, &g0, bw, |_| (1.0, 1.0));
    assert!(vfirst.full);
    vkey.bake_t0 = vfirst.bake_t0;
    vkey.time_to_px = g0.time_to_px;
    vkey.volume_alpha = g0.volume_alpha;
    vkey.scale = vfirst.scale;
    vkey.valid = true;

    let (mut old_bakes, mut new_bakes, mut vol_bakes) = (0, 0, 0);
    let mut prev = g0;
    let mut blit_miss = None;
    for i in 1..=120 {
        now += 16.0;
        // Last price walks up 25 (125 px at 5 px/price), under the 150 px margin.
        let last = 100.0 + 25.0 * i as f32 / 120.0;
        view.update_y(now, bh, Some((last - 50.0, last + 50.0)), Some(last));
        let g = gpu_of(&view, area);
        assert_eq!(
            g.price_to_px.to_bits(),
            g0.price_to_px.to_bits(),
            "the drive must stay inside the range hysteresis"
        );
        // OLD rule: any view_price0 or price_to_px bit change rebaked.
        if g.view_price0.to_bits() != prev.view_price0.to_bits()
            || g.price_to_px.to_bits() != prev.price_to_px.to_bits()
        {
            old_bakes += 1;
        }
        prev = g;
        let plan = plan_cross_bake(&key, &g, bw);
        if plan.full {
            new_bakes += 1;
            commit_cross(&mut key, plan, &g);
        }
        if plan_volume_bake(&vkey, &g, bw, |_| (1.0, 1.0)).full {
            vol_bakes += 1;
        }
        let (off, _) = cross_blit_uv(&key, &g);
        let v_top = off[1] * tex_h_total as f32;
        let want = v_margin - (g.view_price0 - g0.view_price0) * g.price_to_px;
        if blit_miss.is_none() && (v_top - want).abs() > 1.0 {
            blit_miss = Some((i, v_top, want));
        }
    }
    println!("[G1] combo full bakes per 120 follow syncs: old={old_bakes} new={new_bakes}");
    assert!(old_bakes > 0, "the drive must actually move the price axis");
    assert_eq!(new_bakes, 0, "cross bitmap rebaked on price-axis motion");
    assert_eq!(vol_bakes, 0, "volume bitmap rebaked on price-axis motion");
    assert_eq!(
        blit_miss, None,
        "blit v-offset must follow the view's pixel motion"
    );

    // A manual pan inside the margin shifts the blit by exactly its pixels, without a bake.
    let before = cross_blit_uv(&key, &gpu_of(&view, area)).0[1] * tex_h_total as f32;
    view.pan_y_px(20.0, now);
    let g = gpu_of(&view, area);
    assert!(!plan_cross_bake(&key, &g, bw).full, "a 20 px pan bakes");
    let after = cross_blit_uv(&key, &g).0[1] * tex_h_total as f32;
    assert!(
        ((before - after) - 20.0).abs() <= 1.0,
        "pan moved the blit {}",
        before - after
    );

    // Past the margin, or on a new price scale, the bitmap does bake.
    let mut far = g;
    far.view_price0 = key.bake_p0 + (2.0 * v_margin) / g.price_to_px;
    assert!(
        plan_cross_bake(&key, &far, bw).full,
        "drift past the margin must bake"
    );
    let mut zoom = g;
    zoom.price_to_px *= 1.5;
    assert!(
        plan_cross_bake(&key, &zoom, bw).full,
        "a price-scale change must bake"
    );
}
