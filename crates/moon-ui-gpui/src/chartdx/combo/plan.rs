//! Backend-free bake decisions for the combo bitmaps: when a texture must be fully rebaked and
//! which texel window of it the blit shows. The GPU wrappers in `combo.rs` (DX11) and
//! `metal_backend.rs` (Metal) only execute them.

use moon_chart::tick_volume::tick_touches_bake;

use super::super::types::{ChartCross, ChartViewGpu};

/// Horizontal bake margin in pixels, baked on EACH side of the visible chart width.
pub(super) fn combo_x_margin_px(bw: f32) -> f32 {
    (bw * 0.2).max(128.0)
}

/// Bitmap width in texels: the visible width plus a margin on the left and on the right.
pub(super) fn combo_tex_w(bw: f32) -> u32 {
    (bw + 2.0 * combo_x_margin_px(bw)).round().max(1.0) as u32
}

/// Left time edge of a full bake: one margin left of the view, snapped to the texel phase.
fn full_bake_t0(view: &ChartViewGpu, bw: f32) -> f32 {
    let ttp = view.time_to_px;
    let t0 = if ttp > 1e-9 {
        view.view_time0 - combo_x_margin_px(bw) / ttp
    } else {
        view.view_time0
    };
    texel_aligned_time0(t0, ttp)
}

/// Vertical bake margin in pixels above and below the visible chart height.
pub(super) fn combo_v_margin_px(bh: f32) -> f32 {
    (0.25 * bh).max(128.0).round()
}

/// Volume band height in pixels, covering the tallest bar the shader draws plus rounding rows.
pub(super) fn volume_band_px(bh: f32) -> u32 {
    ((bh * 0.18).min(72.0).ceil() + 2.0).max(1.0) as u32
}

/// Snap a bake's left time edge to the global texel phase so rebakes never shift history.
pub(super) fn texel_aligned_time0(time0: f32, time_to_px: f32) -> f32 {
    if !(time_to_px > 1e-9) {
        return time0;
    }
    (time0 * time_to_px).floor() / time_to_px
}

/// State the cross bitmap was baked with; `valid == false` forces the next bake to be full.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ComboBakeKey {
    pub tex_w: u32,
    pub tex_h_total: u32,
    pub v_margin: f32,
    pub bake_t0: f32,
    pub bake_p0: f32,
    pub time_to_px: f32,
    pub price_to_px: f32,
    pub marker_half: f32,
    pub valid: bool,
}

impl ComboBakeKey {
    /// A key that matches no view, for a freshly created texture.
    pub fn unbaked(tex_w: u32, tex_h_total: u32, v_margin: f32) -> Self {
        Self {
            tex_w,
            tex_h_total,
            v_margin,
            bake_t0: 0.0,
            bake_p0: f32::NAN,
            time_to_px: f32::NAN,
            price_to_px: f32::NAN,
            marker_half: f32::NAN,
            valid: false,
        }
    }

    /// Vertical drift of the view from the bake centre, in pixels; zero right after a bake.
    pub fn d_px(&self, view: &ChartViewGpu) -> f64 {
        (f64::from(view.view_price0) - f64::from(self.bake_p0)) * f64::from(view.price_to_px)
            - f64::from(self.v_margin)
    }
}

/// Whether the view has left the margins a bake starting at `bake_t0` covers on either side.
fn x_margin_exhausted(bake_t0: f32, view: &ChartViewGpu, bw: f32) -> bool {
    let u_left_px = (view.view_time0 - bake_t0) * view.time_to_px;
    !(u_left_px >= 0.0 && u_left_px <= 2.0 * combo_x_margin_px(bw) - 1.0)
}

/// Whole-texel left edge of the view inside a bitmap baked from `bake_t0`, as a U coordinate.
fn x_blit_u(bake_t0: f32, tex_w: u32, view: &ChartViewGpu) -> f32 {
    let tex_w = tex_w as f32;
    let u_left_px = ((view.view_time0 - bake_t0) * view.time_to_px)
        .round()
        .clamp(0.0, (tex_w - view.bounds[2]).max(0.0).floor());
    u_left_px / tex_w
}

/// Whether the cross bitmap needs a full bake, and the origin that bake uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CrossBakePlan {
    pub full: bool,
    pub bake_t0: f32,
    pub bake_p0: f32,
}

/// Decide the cross bake: X margin, zoom, marker size or a vertical drift past the margin.
pub(super) fn plan_cross_bake(key: &ComboBakeKey, view: &ChartViewGpu, bw: f32) -> CrossBakePlan {
    let ttp = view.time_to_px;
    let d_px = key.d_px(view);
    let full = !key.valid
        || key.time_to_px.to_bits() != ttp.to_bits()
        || key.price_to_px.to_bits() != view.price_to_px.to_bits()
        || key.marker_half.to_bits() != view.marker_half.to_bits()
        || x_margin_exhausted(key.bake_t0, view, bw)
        || !(d_px.abs() <= f64::from(key.v_margin) - 1.0);
    if !full {
        return CrossBakePlan {
            full,
            bake_t0: key.bake_t0,
            bake_p0: key.bake_p0,
        };
    }
    let bake_p0 = if view.price_to_px > 1e-12 {
        view.view_price0 - key.v_margin / view.price_to_px
    } else {
        view.view_price0
    };
    CrossBakePlan {
        full,
        bake_t0: full_bake_t0(view, bw),
        bake_p0,
    }
}

/// UV window of the cross bitmap for this view, in whole texels on both axes.
pub(super) fn cross_blit_uv(key: &ComboBakeKey, view: &ChartViewGpu) -> ([f32; 2], [f32; 2]) {
    let bw = view.bounds[2];
    let bh = view.bounds[3];
    let tex_w = key.tex_w as f32;
    let tex_h = key.tex_h_total as f32;
    let v_top_px = (f64::from(key.v_margin) - key.d_px(view).round())
        .clamp(0.0, f64::from((tex_h - bh).max(0.0).floor())) as f32;
    (
        [x_blit_u(key.bake_t0, key.tex_w, view), v_top_px / tex_h],
        [bw / tex_w, bh / tex_h],
    )
}

/// State the volume band bitmap was baked with; it has no price axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct VolumeBakeKey {
    pub tex_w: u32,
    pub band_px: u32,
    pub chart_h: u32,
    pub bake_t0: f32,
    pub time_to_px: f32,
    pub volume_alpha: f32,
    pub scale: (f32, f32),
    pub valid: bool,
}

impl VolumeBakeKey {
    /// A key that matches no view, for a freshly created texture.
    pub fn unbaked(tex_w: u32, band_px: u32, chart_h: u32) -> Self {
        Self {
            tex_w,
            band_px,
            chart_h,
            bake_t0: 0.0,
            time_to_px: f32::NAN,
            volume_alpha: f32::NAN,
            scale: (f32::NAN, f32::NAN),
            valid: false,
        }
    }
}

/// Whether the volume bitmap needs a full bake, its origin, and the bar scale it bakes with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct VolumeBakePlan {
    pub full: bool,
    pub bake_t0: f32,
    pub scale: (f32, f32),
}

/// Decide the volume bake from X motion, zoom, opacity and the bake window's bar scale; the
/// price axis never enters. `scale_for` resolves the scale of the window starting at a time.
pub(super) fn plan_volume_bake(
    key: &VolumeBakeKey,
    view: &ChartViewGpu,
    bw: f32,
    mut scale_for: impl FnMut(f32) -> (f32, f32),
) -> VolumeBakePlan {
    let ttp = view.time_to_px;
    let mut full = !key.valid
        || key.time_to_px.to_bits() != ttp.to_bits()
        || key.volume_alpha.to_bits() != view.volume_alpha.to_bits()
        || x_margin_exhausted(key.bake_t0, view, bw);
    let bake_t0 = if full {
        full_bake_t0(view, bw)
    } else {
        key.bake_t0
    };
    let scale = scale_for(bake_t0);
    if scale != key.scale {
        full = true;
    }
    VolumeBakePlan {
        full,
        bake_t0,
        scale,
    }
}

/// UV window of the volume band bitmap: whole-texel X pan, the full band height.
pub(super) fn volume_blit_uv(key: &VolumeBakeKey, view: &ChartViewGpu) -> ([f32; 2], [f32; 2]) {
    (
        [x_blit_u(key.bake_t0, key.tex_w, view), 0.0],
        [view.bounds[2] / key.tex_w as f32, 1.0],
    )
}

/// What an applied tick append does to the baked bitmaps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AppendBakeDamage {
    /// Only new rows land; the incremental pass draws them.
    None,
    /// Rows evicted from a baked span stay painted until one coalesced rebake.
    Defer,
    /// The ring was re-uploaded whole, so both bitmaps rebake on the next prepare.
    Invalidate,
}

/// Classify an append: a refused in-place write or a capacity-sized batch (`written == false`)
/// invalidates at once; eviction of possibly baked rows alone is deferred.
pub(super) fn append_bake_damage(written: bool, evicts_baked: bool) -> AppendBakeDamage {
    if !written {
        AppendBakeDamage::Invalidate
    } else if evicts_baked {
        AppendBakeDamage::Defer
    } else {
        AppendBakeDamage::None
    }
}


/// Whether new rows, or the old rows they evict, intersect either cached span.
///
/// `old` is the logical ring before the append, in chronological order. The
/// eviction count is how far `old.len() + new_rows.len()` passes `capacity`,
/// and it never exceeds `old.len()`. A non-finite time touches every span.
///
/// Args:
///     old: Pending logical ring before this append.
///     new_rows: Rows about to be appended.
///     cross_span: Cached cross-bitmap time span, margins included.
///     volume_span: Cached volume-bitmap time span, margins included.
///     capacity: Ring capacity. Already a positive normalized value.
///
/// Returns:
///     `true` when any tested time lies in either span.
pub(super) fn append_span_damage<'a>(
    old: impl ExactSizeIterator<Item = &'a ChartCross>,
    new_rows: &[ChartCross],
    cross_span: (f64, f64),
    volume_span: (f64, f64),
    capacity: usize,
) -> bool {
    let evicted = old
        .len()
        .saturating_add(new_rows.len())
        .saturating_sub(capacity)
        .min(old.len());
    let touches =
        |time: f32| tick_touches_bake(time, cross_span) || tick_touches_bake(time, volume_span);
    new_rows.iter().any(|row| touches(row.time_rel))
        || old.take(evicted).any(|row| touches(row.time_rel))
}
