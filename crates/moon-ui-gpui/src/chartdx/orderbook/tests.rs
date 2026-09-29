//! CPU-only checks for the backend-free order-book bake planner shared by DX11 and Metal.

use std::time::{Duration, Instant};

use super::plan::{
    BOOK_DATA_THROTTLE, BookBakeKey, book_blit_uv, book_data_due, book_tex_dims, plan_book_bake,
};
use crate::chartdx::types::ChartViewGpu;

fn view(price0: f32, price_to_px: f32, bh: f32) -> ChartViewGpu {
    ChartViewGpu {
        bounds: [0.0, 0.0, 200.0, bh],
        resolution: [200.0, bh],
        time_to_px: 1.0,
        view_time0: 0.0,
        price_to_px,
        view_price0: price0,
        marker_half: 0.0,
        pad: 0.0,
        volume_buy_inv: 0.0,
        volume_sell_inv: 0.0,
        volume_alpha: 0.0,
        _pad2: 0.0,
    }
}

/// A key baked centred on `v`, as a bake at that view leaves it.
fn baked_at(v: &ChartViewGpu) -> BookBakeKey {
    let (_, v_margin, tex_h_total) = book_tex_dims(v.bounds[2], v.bounds[3]);
    let mut key = BookBakeKey::unbaked(tex_h_total, v_margin);
    key.bake_p0 = key.centred_p0(v);
    key.price_to_px = v.price_to_px;
    key.baked = true;
    key
}

/// Breakage this pins: dropping the throttle rebakes the book on every revision (several per
/// second), and a throttle that also held the first bake would show an empty zone.
#[test]
fn data_bakes_at_most_once_per_throttle_interval() {
    let t0 = Instant::now();
    assert!(book_data_due(true, None, t0), "the first data bake is due");
    assert!(!book_data_due(false, None, t0), "clean data never bakes");
    let almost = t0 + BOOK_DATA_THROTTLE - Duration::from_millis(1);
    assert!(!book_data_due(true, Some(t0), almost));
    assert!(book_data_due(true, Some(t0), t0 + BOOK_DATA_THROTTLE));
}

/// Breakage this pins: a margin-less bitmap, or a planner that rebakes on any Y move, rebuilds
/// the book on every vertical pan; a drift past the margin must still bake at once.
#[test]
fn a_price_shift_inside_the_margin_moves_the_window_without_a_bake() {
    let v0 = view(100.0, 10.0, 400.0);
    let key = baked_at(&v0);
    assert_eq!(key.v_margin, 128.0);
    assert_eq!(key.tex_h_total, 400 + 2 * 128);
    assert_eq!(book_blit_uv(&key, &v0).0[1], 128.0 / key.tex_h_total as f32);

    // 5 px up inside the margin: no bake, the UV window moves by whole texels.
    let shifted = view(100.5, 10.0, 400.0);
    let plan = plan_book_bake(&key, &shifted, false, false, false);
    assert!(!plan.bake);
    let (uv_off, uv_scale) = book_blit_uv(&key, &shifted);
    assert_eq!(uv_off[1], 123.0 / key.tex_h_total as f32);
    assert_eq!(uv_scale[1], 400.0 / key.tex_h_total as f32);

    // Past the margin: an immediate bake.
    let far = view(100.0 + 130.0 / 10.0, 10.0, 400.0);
    let plan = plan_book_bake(&key, &far, false, false, false);
    assert!(plan.bake && plan.immediate);
}

/// Breakage this pins: throttling a zoom, a hard style change or a window rebuild lets the book
/// lag the chart; data alone must wait for the throttle.
#[test]
fn only_data_waits_for_the_throttle() {
    let v0 = view(100.0, 10.0, 400.0);
    let key = baked_at(&v0);
    let data = plan_book_bake(&key, &v0, false, true, false);
    assert!(data.bake && !data.immediate);
    assert!(!plan_book_bake(&key, &v0, false, false, false).bake);
    assert!(plan_book_bake(&key, &view(100.0, 11.0, 400.0), false, false, false).immediate);
    assert!(plan_book_bake(&key, &v0, true, false, false).immediate);
    assert!(plan_book_bake(&key, &v0, false, false, true).immediate);
    let unbaked = BookBakeKey::unbaked(key.tex_h_total, key.v_margin);
    assert!(plan_book_bake(&unbaked, &v0, false, false, false).immediate);
}
