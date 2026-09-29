//! Backend-free bake decisions for the order-book bitmap: its size, when it must bake, whether the
//! data throttle lets a bake through, and which texel window of it the blit shows. The GPU
//! wrappers in `orderbook.rs` (DX11) and `metal_backend.rs` (Metal) only execute them.

use std::time::{Duration, Instant};

use super::super::types::ChartViewGpu;

/// Minimum interval between two data-driven bakes, like Moonbot's `bmGlass` at about 5 Hz.
pub(crate) const BOOK_DATA_THROTTLE: Duration = Duration::from_millis(200);

/// Vertical bake margin in pixels above and below the visible order-book zone.
pub fn book_v_margin_px(bh: f32) -> f32 {
    (0.25 * bh).max(128.0).round()
}

/// Bitmap size for a zone of `bw x bh`: `(tex_w, v_margin, tex_h_total)`, the height carrying a
/// margin on each side.
pub(crate) fn book_tex_dims(bw: f32, bh: f32) -> (u32, f32, u32) {
    let v_margin = book_v_margin_px(bh);
    let tex_h_total = bh.round().max(1.0) as u32 + 2 * v_margin as u32;
    (bw.round().max(1.0) as u32, v_margin, tex_h_total)
}

/// Whether changed book data may bake now: only once `BOOK_DATA_THROTTLE` has passed since the
/// previous bake.
pub(crate) fn book_data_due(dirty: bool, last_bake_at: Option<Instant>, now: Instant) -> bool {
    dirty && last_bake_at.is_none_or(|last| now.duration_since(last) >= BOOK_DATA_THROTTLE)
}

/// Y state the book bitmap was baked with; `baked == false` forces an immediate bake.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BookBakeKey {
    pub tex_h_total: u32,
    pub v_margin: f32,
    pub bake_p0: f32,
    pub price_to_px: f32,
    pub baked: bool,
}

impl BookBakeKey {
    /// A key that matches no view, for a freshly created texture.
    pub fn unbaked(tex_h_total: u32, v_margin: f32) -> Self {
        Self {
            tex_h_total,
            v_margin,
            bake_p0: f32::NAN,
            price_to_px: f32::NAN,
            baked: false,
        }
    }

    /// Vertical drift of the view from the bake centre, in pixels; zero right after a bake.
    pub fn d_px(&self, view: &ChartViewGpu) -> f64 {
        (f64::from(view.view_price0) - f64::from(self.bake_p0)) * f64::from(view.price_to_px)
            - f64::from(self.v_margin)
    }

    /// Bake origin price that centres a view in the margin.
    pub fn centred_p0(&self, view: &ChartViewGpu) -> f32 {
        if view.price_to_px > 1e-12 {
            view.view_price0 - self.v_margin / view.price_to_px
        } else {
            view.view_price0
        }
    }
}

/// Whether to bake this frame, and whether that bake bypasses the data throttle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BookBakePlan {
    pub bake: bool,
    pub immediate: bool,
}

/// Decide the book bake: price scale, hard style, a drift past the margin, the first bake or a
/// level rebuild caused by leaving the emitted window bake now; other data waits for `data_due`.
pub(crate) fn plan_book_bake(
    key: &BookBakeKey,
    view: &ChartViewGpu,
    style_hard_changed: bool,
    data_due: bool,
    window_rebuilt: bool,
) -> BookBakePlan {
    let immediate = !key.baked
        || key.price_to_px.to_bits() != view.price_to_px.to_bits()
        || style_hard_changed
        || !(key.d_px(view).abs() <= f64::from(key.v_margin) - 1.0)
        || window_rebuilt;
    BookBakePlan {
        bake: immediate || data_due,
        immediate,
    }
}

/// UV window of the book bitmap for this view: full width, whole-texel vertical offset.
pub(crate) fn book_blit_uv(key: &BookBakeKey, view: &ChartViewGpu) -> ([f32; 2], [f32; 2]) {
    let bh = view.bounds[3];
    let tex_h = key.tex_h_total as f32;
    let v_top_px = (f64::from(key.v_margin) - key.d_px(view).round())
        .clamp(0.0, f64::from((tex_h - bh).max(0.0).floor())) as f32;
    ([0.0, v_top_px / tex_h], [1.0, bh / tex_h])
}
