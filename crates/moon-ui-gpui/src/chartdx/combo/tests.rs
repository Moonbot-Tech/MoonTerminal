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
        assert!(!layer.append_evicts_baked(layer.pending_append.len(), span));
        assert!(
            layer.append_evicts_baked(3, span),
            "visible eviction must repaint"
        );
        assert!(
            layer.append_evicts_baked(4, span),
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
        ComboBakeKey, VolumeBakeKey, combo_tex_w, combo_v_margin_px, cross_blit_uv,
        plan_cross_bake, plan_volume_bake,
    };
    let (mut view, area, mut now) = follow_pane();
    let (bw, bh) = (area.w, area.h);
    let v_margin = combo_v_margin_px(bh);
    let tex_w = combo_tex_w(bw);
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

/// Whether `time` lies inside a cached bitmap span. Non-finite times are outside this fixture.
///
/// Args:
///     time: Row time, relative to the chart epoch.
///     span: Inclusive bake interval.
///
/// Returns:
///     `true` when a finite time is inside the span.
fn covers(time: f32, span: (f64, f64)) -> bool {
    time.is_finite() && f64::from(time) >= span.0 && f64::from(time) <= span.1
}

/// Chronological rows the next upload would publish, copied out of the pending ring.
///
/// Args:
///     layer: Combo layer whose reset and append queues are already filled.
///
/// Returns:
///     The retained logical ring, oldest first.
fn logical_ring(layer: &ComboLayer) -> Vec<ChartCross> {
    let capacity = layer.cross_capacity as usize;
    moon_chart::tick_volume::pending_ring(
        &layer.resident_crosses,
        layer.resident_head,
        layer.resident_count,
        capacity,
        layer.pending_reset.as_deref(),
        &layer.pending_append,
    )
    .copied()
    .collect()
}

/// Run the damage predicate on `new_rows` without appending them.
///
/// Args:
///     layer: Pending ring the predicate reads.
///     new_rows: Batch that has not been queued yet.
///     cross_span: Cached cross-bitmap span.
///     volume_span: Cached volume-bitmap span.
///
/// Returns:
///     Whatever `append_span_damage` reports for that ring.
fn damage_of(
    layer: &ComboLayer,
    new_rows: &[ChartCross],
    cross_span: (f64, f64),
    volume_span: (f64, f64),
) -> bool {
    let capacity = layer.cross_capacity as usize;
    let old = moon_chart::tick_volume::pending_ring(
        &layer.resident_crosses,
        layer.resident_head,
        layer.resident_count,
        capacity,
        layer.pending_reset.as_deref(),
        &layer.pending_append,
    );
    super::append_span_damage(old, new_rows, cross_span, volume_span, capacity)
}

/// `combo.rs:append_span_damage` deleting the evicted-prefix disjunct still accepts offscreen
/// new rows and leaves a visible cross or volume bar baked after the ring has dropped it.
#[test]
fn offscreen_rows_that_evict_a_visible_row_damage_the_cached_span() {
    let cross_span = (1_000.0, 1_100.0);
    let volume_span = (90.0, 160.0);
    let capacity = 4usize;
    let new_rows = [cross(500.0, 0, 1.0), cross(600.0, 0, 1.0)];

    let mut quiet = ComboLayer::new();
    quiet.set_capacity(capacity, 1);
    quiet.reset(vec![
        cross(10.0, 0, 1.0),
        cross(20.0, 0, 1.0),
        cross(300.0, 0, 1.0),
    ]);
    quiet.append(&[cross(400.0, 0, 1.0)]);
    assert!(
        !damage_of(&quiet, &new_rows, cross_span, volume_span),
        "offscreen eviction of offscreen rows must not repaint"
    );

    let mut layer = ComboLayer::new();
    layer.set_capacity(capacity, 1);
    // Pending reset replaces resident rows. The queued append is already part of the old ring.
    // Times 100 and 150 sit in the volume span and miss the cross span.
    layer.reset(vec![
        cross(100.0, 0, 1.0),
        cross(150.0, 0, 1.0),
        cross(300.0, 0, 1.0),
    ]);
    layer.append(&[cross(400.0, 0, 1.0)]);

    let old_rows = logical_ring(&layer);
    let mut combined = old_rows.clone();
    combined.extend_from_slice(&new_rows);
    let retained = &combined[combined.len().saturating_sub(capacity)..];
    let evicted_n = old_rows
        .len()
        .saturating_add(new_rows.len())
        .saturating_sub(capacity)
        .min(old_rows.len());
    let evicted = &old_rows[..evicted_n];

    assert!(
        evicted
            .iter()
            .any(|row| { covers(row.time_rel, volume_span) && !covers(row.time_rel, cross_span) }),
        "fixture must evict a volume-only row"
    );
    assert!(
        new_rows
            .iter()
            .all(|row| { !covers(row.time_rel, volume_span) && !covers(row.time_rel, cross_span) }),
        "new rows must miss both cached spans"
    );
    assert!(
        evicted
            .iter()
            .all(|row| retained.iter().all(|kept| kept.price != row.price)),
        "evicted prices must leave the retained ring"
    );

    assert!(
        damage_of(&layer, &new_rows, cross_span, volume_span),
        "evicted visible row must mark cached volume damage"
    );
}

/// `combo.rs:append_span_damage` dropping `new_rows.iter().any(...)` ignores a tick that lands
/// in a cached span when the ring does not evict.
///
/// The user-visible consequence is a stale cross or volume bitmap until a later eviction.
/// A miss outside both spans, with no eviction, stays quiet.
#[test]
fn new_row_in_span_damages_without_eviction() {
    let cross_span = (1_000.0, 1_100.0);
    let volume_span = (90.0, 160.0);
    let mut layer = ComboLayer::new();
    layer.set_capacity(8, 1);
    layer.reset(vec![
        cross(10.0, 0, 1.0),
        cross(20.0, 0, 1.0),
        cross(30.0, 0, 1.0),
    ]);
    let old = logical_ring(&layer);
    assert!(old.len() < 8, "fixture must not evict");
    assert!(
        damage_of(&layer, &[cross(120.0, 0, 4.0)], cross_span, volume_span),
        "a new row inside the volume span must damage without eviction"
    );
    assert!(
        damage_of(&layer, &[cross(1_050.0, 0, 5.0)], cross_span, volume_span),
        "a new row inside the cross span must damage without eviction"
    );
    assert!(
        !damage_of(&layer, &[cross(5_000.0, 0, 6.0)], cross_span, volume_span),
        "a new row outside both spans must stay quiet when nothing is evicted"
    );
}

/// `combo.rs:ComboLayer::append` replacing `drain(..excess)` with `truncate(cap)` keeps the
/// oldest queued rows and drops the live tail.
///
/// The user-visible consequence is the chart drawing stale ticks after the queue overflows.
/// Lateness is evidence over every appended time, so the trim must not clear it.
#[test]
fn pending_append_keeps_newest_capacity() {
    let cap = 4usize;
    let mut layer = ComboLayer::new();
    layer.set_capacity(cap, 1);
    let mut all = Vec::new();
    all.push(cross(50.0, 0, 1.0));
    all.push(cross(10.0, 0, 1.0));
    for i in 0..(cap * 2) {
        all.push(cross(100.0 + i as f32, 0, 1.0));
    }
    assert!(all.len() > cap * 2, "fixture must cross the overflow line");
    layer.append(&all);
    let tail = &all[all.len() - cap..];
    assert_eq!(
        layer.pending_append.len(),
        cap,
        "overflow keeps one capacity"
    );
    for (got, want) in layer.pending_append.iter().zip(tail.iter()) {
        assert_eq!(got.time_rel, want.time_rel, "oldest rows must not survive");
        assert_eq!(got.price, want.price);
    }
    let mut order = moon_chart::tick_volume::TickTimeOrder::default();
    order.extend(all.iter().map(|row| row.time_rel));
    assert_eq!(
        layer.tick_time_order.max_lateness(),
        order.max_lateness(),
        "overflow must keep lateness from the dropped prefix"
    );
    assert!(
        order.max_lateness() >= 40.0,
        "fixture lateness is 50 minus 10"
    );
}

/// `combo/plan.rs:full_bake_t0` / `x_margin_exhausted`: a bake that starts at the view's left
/// edge again (a right-only margin) fully rebakes both bitmaps on the first pixel of a drag
/// toward history; a pan either way inside the margin must only slide the blit.
#[test]
fn horizontal_pan_inside_the_margin_either_way_never_rebakes() {
    use super::plan::{
        ComboBakeKey, VolumeBakeKey, combo_tex_w, combo_v_margin_px, combo_x_margin_px,
        cross_blit_uv, plan_cross_bake, plan_volume_bake, volume_blit_uv,
    };
    let (view, area, _) = follow_pane();
    let (bw, bh) = (area.w, area.h);
    let margin = combo_x_margin_px(bw);
    let tex_w = combo_tex_w(bw);
    assert_eq!(tex_w, (bw + 2.0 * margin).round() as u32);
    let v_margin = combo_v_margin_px(bh);
    let mut key = ComboBakeKey::unbaked(tex_w, bh as u32 + 2 * v_margin as u32, v_margin);
    let mut vkey = VolumeBakeKey::unbaked(tex_w, 72, bh as u32);
    let g0 = gpu_of(&view, area);
    let first = plan_cross_bake(&key, &g0, bw);
    assert!(first.full);
    commit_cross(&mut key, first, &g0);
    let vfirst = plan_volume_bake(&vkey, &g0, bw, |_| (1.0, 1.0));
    assert!(vfirst.full);
    assert_eq!(vfirst.bake_t0.to_bits(), first.bake_t0.to_bits());
    vkey.bake_t0 = vfirst.bake_t0;
    vkey.time_to_px = g0.time_to_px;
    vkey.volume_alpha = g0.volume_alpha;
    vkey.scale = vfirst.scale;
    vkey.valid = true;

    // The bake leaves a whole margin of history to the left of the view.
    let left_px = (g0.view_time0 - first.bake_t0) * g0.time_to_px;
    assert!(
        left_px >= margin - 1.0 && left_px <= margin + 1.0,
        "left margin {left_px} px, want {margin}"
    );
    let u0 = cross_blit_uv(&key, &g0).0[0] * tex_w as f32;
    let reach = (margin - 2.0).floor() as i32;
    for px in [-reach, -40, -1, 1, 40, reach] {
        let mut g = g0;
        g.view_time0 = g0.view_time0 + px as f32 / g0.time_to_px;
        assert!(
            !plan_cross_bake(&key, &g, bw).full,
            "{px} px pan rebakes crosses"
        );
        assert!(
            !plan_volume_bake(&vkey, &g, bw, |_| (1.0, 1.0)).full,
            "{px} px pan rebakes volume"
        );
        let u = cross_blit_uv(&key, &g).0[0] * tex_w as f32;
        assert!(
            ((u - u0) - px as f32).abs() <= 1.0,
            "{px} px pan moved the blit {}",
            u - u0
        );
        let vu = volume_blit_uv(&vkey, &g).0[0] * tex_w as f32;
        assert!((vu - u).abs() < 1e-3, "volume and crosses blit apart");
    }
    for px in [-(margin as i32) - 4, margin as i32 + 4] {
        let mut g = g0;
        g.view_time0 = g0.view_time0 + px as f32 / g0.time_to_px;
        assert!(
            plan_cross_bake(&key, &g, bw).full,
            "{px} px pan past the margin"
        );
        assert!(plan_volume_bake(&vkey, &g, bw, |_| (1.0, 1.0)).full);
    }
}

/// `ComboLayer::defer_eviction_rebake` / `flush_due_eviction`: arming the rebake on every
/// eviction, or pushing the deadline back on each one, either rebakes per tick or never.
#[test]
fn eviction_damage_waits_for_one_coalesced_rebake() {
    use std::time::{Duration, Instant};
    let mut layer = ComboLayer::new();
    let t0 = Instant::now();
    assert!(!layer.eviction_rebake_due(t0 + Duration::from_secs(60)));
    layer.defer_eviction_rebake(t0);
    assert!(!layer.eviction_rebake_due(t0));
    layer.defer_eviction_rebake(t0 + Duration::from_millis(600));
    let almost = t0 + super::EVICTION_REBAKE_INTERVAL - Duration::from_millis(1);
    assert!(
        !layer.eviction_rebake_due(almost),
        "due before the interval"
    );
    layer.flush_due_eviction(almost);
    assert!(
        layer.eviction_rebake_at.is_some(),
        "flushed before the interval"
    );
    let due = t0 + super::EVICTION_REBAKE_INTERVAL;
    assert!(
        layer.eviction_rebake_due(due),
        "a later eviction pushed the deadline"
    );
    layer.flush_due_eviction(due);
    assert!(
        !layer.eviction_rebake_due(due + Duration::from_secs(5)),
        "flush kept the deadline"
    );

    // Eviction alone defers; a refused write or a capacity-sized batch re-uploads the ring and
    // invalidates at once, which also drops a pending deadline.
    use super::plan::{AppendBakeDamage, append_bake_damage};
    assert_eq!(append_bake_damage(true, false), AppendBakeDamage::None);
    assert_eq!(append_bake_damage(true, true), AppendBakeDamage::Defer);
    assert_eq!(
        append_bake_damage(false, false),
        AppendBakeDamage::Invalidate
    );
    assert_eq!(
        append_bake_damage(false, true),
        AppendBakeDamage::Invalidate
    );
    layer.settle_append_damage(AppendBakeDamage::Defer, due);
    assert!(layer.eviction_rebake_due(due + super::EVICTION_REBAKE_INTERVAL));
    layer.settle_append_damage(AppendBakeDamage::Invalidate, due);
    assert_eq!(layer.eviction_rebake_at, None);

    // Any other immediate invalidation (here a style change) erases it too.
    layer.defer_eviction_rebake(due);
    let style = super::TickStyleGpu {
        buy: [0.5; 4],
        ..Default::default()
    };
    layer.set_tick_style(style);
    assert_eq!(layer.eviction_rebake_at, None);
}
