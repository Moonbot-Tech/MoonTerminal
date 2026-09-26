//! Order-book layer with its OWN right-side zone, independent of the time series and combo layer.
//! The zone background, cumulative depth-fill rectangles, and individual level lines bake into an
//! offscreen `BookTex`, analogous to Moonbot's `bmGlass`. During an outer `BaseCache` rebuild,
//! `BookTex` is composited into that cache; `BaseCache`, not `BookTex`, blits on each present. The
//! texture is taller than the zone by a vertical margin on each side, so a pure price shift only
//! moves its UV window; it rebakes for level data, price scale, a drift past the margin, non-edge
//! style, size, or device-generation changes, while live edge movement uses the throttled path.

use std::time::{Duration, Instant};

use gpui::RawGpuAccess;
use moon_core::data::LevelInstance;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::gpu::{
    BlitParams, ChartViewGpu, create_alpha_blend, create_dynamic_cb, create_point_sampler,
    create_srv, create_structured, device_changed, full_viewport, make_ps, make_vs,
    set_scissor_rect, update_dynamic,
};
pub use super::types::BookStyle;

const BARS_HLSL: &str = include_str!("shaders/bars.hlsl");
const BLIT_HLSL: &str = include_str!("shaders/blit.hlsl");
const INITIAL_LEVEL_BUFFER_CAPACITY: u32 = 256;

/// Vertical bake margin in pixels above and below the visible order-book zone.
pub fn book_v_margin_px(bh: f32) -> f32 {
    (0.25 * bh).max(128.0).round()
}

/// Y state the book bitmap was baked with; `baked == false` forces an immediate bake.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct BookBakeKey {
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
pub(super) struct BookBakePlan {
    pub bake: bool,
    pub immediate: bool,
}

/// Decide the book bake: price scale, hard style, a drift past the margin, the first bake or a
/// level rebuild caused by leaving the emitted window bake now; other data waits for `data_due`.
pub(super) fn plan_book_bake(
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
pub(super) fn book_blit_uv(key: &BookBakeKey, view: &ChartViewGpu) -> ([f32; 2], [f32; 2]) {
    let bh = view.bounds[3];
    let tex_h = key.tex_h_total as f32;
    let v_top_px = (f64::from(key.v_margin) - key.d_px(view).round())
        .clamp(0.0, f64::from((tex_h - bh).max(0.0).floor())) as f32;
    ([0.0, v_top_px / tex_h], [1.0, bh / tex_h])
}

struct BookPipe {
    bars_vs: ID3D11VertexShader,
    bars_ps: ID3D11PixelShader,
    bg_vs: ID3D11VertexShader,
    bg_ps: ID3D11PixelShader,
    blend: ID3D11BlendState,
    buffer: ID3D11Buffer,
    srv: ID3D11ShaderResourceView,
    level_cap: u32,
    view_cb: ID3D11Buffer,
    style_cb: ID3D11Buffer,
}

/// Offscreen order-book bitmap analogous to `bmGlass`, containing baked background and bars plus
/// validity state and the Y inputs for which the texture remains valid. Its size equals the
/// `glass_area`. The fixed order-book zone does not scroll on X, so it uses a one-to-one blit without
/// UV panning and is simpler than combo.
struct BookTex {
    _tex: ID3D11Texture2D, // RAII keeps the texture referenced by RTV and SRV alive
    rtv: ID3D11RenderTargetView,
    srv: ID3D11ShaderResourceView,
    tex_w: u32,
    blit_vs: ID3D11VertexShader,
    blit_fs: ID3D11PixelShader,
    blit_cb: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    /// Y state of the last bake; `key.baked` is false until the first bake so the book is never
    /// shown black.
    key: BookBakeKey,
    last_style: BookStyle,
    /// Whether inputs changed since the previous bake, requiring a rebake throttled to 200 ms.
    dirty: bool,
    /// Time of the previous bake, throttling rebakes to about 5 Hz like Moonbot `bmGlass` at 200 ms.
    last_bake_at: Option<Instant>,
}

pub struct OrderBookLayer {
    pipe: Option<BookPipe>,
    tex: Option<BookTex>,
    count: u32,
    /// Levels queued by `set`, reused across uploads; `pending_set` marks them unconsumed.
    pending: Vec<LevelInstance>,
    pending_set: bool,
    /// Whether a queued upload came from leaving the emitted window and must bake at once.
    pending_immediate: bool,
    device_generation: u64,
}

impl OrderBookLayer {
    pub fn new() -> Self {
        Self {
            pipe: None,
            tex: None,
            count: 0,
            pending: Vec::new(),
            pending_set: false,
            pending_immediate: false,
            device_generation: 0,
        }
    }

    /// Upload all order-book levels after a book or window change, invalidating the cache.
    /// `immediate` bakes them this frame instead of behind the data throttle.
    pub fn set(&mut self, levels: &[LevelInstance], immediate: bool) {
        self.pending.clear();
        self.pending.extend_from_slice(levels);
        self.pending_set = true;
        self.pending_immediate |= immediate;
    }

    /// Prepare phase: uploads levels and bakes the offscreen book texture when due.
    /// This may switch render targets and must run from `GpuCanvasDriver::prepare_gpu`.
    pub fn prepare(
        &mut self,
        view: &ChartViewGpu,
        style: &BookStyle,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        gpu: &RawGpuAccess,
    ) {
        let bw = view.bounds[2];
        let bh = view.bounds[3];
        if bw <= 0.0 || bh <= 0.0 {
            return;
        }
        // Device loss drops the pipeline and texture and resets the level count. This layer keeps no
        // CPU copy and cannot reupload by itself, so it remains empty until a later `set()` queues
        // fresh levels.
        if device_changed(&mut self.device_generation, gpu) {
            self.pipe = None;
            self.tex = None;
            self.count = 0;
        }
        if self.pipe.is_none() {
            self.pipe = Some(Self::create_pipe(device, INITIAL_LEVEL_BUFFER_CAPACITY));
        }
        // Apply incoming levels and invalidate the texture cache.
        let mut levels_changed = false;
        let mut window_rebuilt = false;
        if std::mem::take(&mut self.pending_set) {
            let levels = &self.pending;
            let need_cap = next_buffer_cap(levels.len(), INITIAL_LEVEL_BUFFER_CAPACITY);
            if self.pipe.as_ref().is_none_or(|p| p.level_cap < need_cap) {
                self.pipe = Some(Self::create_pipe(device, need_cap));
            }
            if !levels.is_empty() {
                let pipe = self.pipe.as_ref().unwrap();
                update_dynamic(context, &pipe.buffer, levels);
            }
            self.count = levels.len() as u32;
            levels_changed = true;
            window_rebuilt = std::mem::take(&mut self.pending_immediate);
        }

        let tex_w = bw.round().max(1.0) as u32;
        let v_margin = book_v_margin_px(bh);
        let tex_h_total = bh.round().max(1.0) as u32 + 2 * v_margin as u32;
        let need_new = self
            .tex
            .as_ref()
            .is_none_or(|t| t.tex_w != tex_w || t.key.tex_h_total != tex_h_total);
        if need_new {
            self.tex = Some(Self::create_tex(device, tex_w, tex_h_total, v_margin));
        }

        let pipe = self.pipe.as_ref().unwrap();
        let count = self.count;
        let tex = self.tex.as_mut().unwrap();
        // Level data and style invalidate the baked image here; the Y transform is the planner's.
        // Texture recreation handles size and device changes; the style checks below distinguish
        // immediate non-edge changes from throttled live-edge movement.
        if levels_changed || *style != tex.last_style {
            tex.dirty = true;
        }
        // Live bid and ask edges move on every book tick and invalidate the bake only through the
        // throttled dirty path above. Any non-edge `BookStyle` field (`eq_ignore_edges`) and the Y
        // transform require an IMMEDIATE rebake.
        let style_hard_changed = !style.eq_ignore_edges(&tex.last_style);

        // BAKE the background and bars into the texture using a texture-local view. Book data may be throttled, but
        // camera/price-transform changes from user pan/zoom must bake immediately; otherwise
        // the chart moves while the glass layer visibly lags behind.
        let now = Instant::now();
        let book_data_due = tex.dirty
            && tex
                .last_bake_at
                .is_none_or(|last| now.duration_since(last) >= Duration::from_millis(200));
        let plan = plan_book_bake(
            &tex.key,
            view,
            style_hard_changed,
            book_data_due,
            window_rebuilt,
        );
        if plan.bake {
            crate::diag::bump(&crate::diag::CHART_BOOK_BAKE);
            let tex_h = tex_h_total;
            let bake_p0 = tex.key.centred_p0(view);
            // Bake view spans the full `[0, 0, tex_w, tex_h_total]` bitmap, margins included.
            let bake_view = ChartViewGpu {
                bounds: [0.0, 0.0, tex_w as f32, tex_h as f32],
                resolution: [tex_w as f32, tex_h as f32],
                time_to_px: view.time_to_px,
                view_time0: view.view_time0,
                price_to_px: view.price_to_px,
                view_price0: bake_p0,
                marker_half: view.marker_half,
                pad: 0.0,
                volume_buy_inv: 0.0,
                volume_sell_inv: 0.0,
                volume_alpha: 0.0,
                _pad2: 0.0,
            };
            update_dynamic(context, &pipe.view_cb, &[bake_view]);
            update_dynamic(context, &pipe.style_cb, &[*style]);
            let tex_vp = D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: tex_w as f32,
                Height: tex_h as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };
            unsafe {
                context.OMSetRenderTargets(Some(&[Some(tex.rtv.clone())]), None);
                context.RSSetViewports(Some(&[tex_vp]));
                set_scissor_rect(context, 0.0, 0.0, tex_w as f32, tex_h as f32);
                context.ClearRenderTargetView(&tex.rtv, &[0.0, 0.0, 0.0, 0.0]);
                context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                context.VSSetConstantBuffers(0, Some(&[Some(pipe.view_cb.clone())]));
                context.VSSetConstantBuffers(1, Some(&[Some(pipe.style_cb.clone())]));
                // The background pixel shader maps price edges through the view's Y transform to ask/bid zones.
                context.PSSetConstantBuffers(0, Some(&[Some(pipe.view_cb.clone())]));
                context.PSSetConstantBuffers(1, Some(&[Some(pipe.style_cb.clone())]));
                context.OMSetBlendState(None, None, 0xFFFFFFFF);
                // Always draw the zone background as opaque `book_bg`, even for an empty book.
                context.VSSetShader(&pipe.bg_vs, None);
                context.PSSetShader(&pipe.bg_ps, None);
                context.Draw(6, 0);
                // Draw fill rectangles and individual level lines.
                if count > 0 {
                    context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
                    context.VSSetShaderResources(1, Some(&[Some(pipe.srv.clone())]));
                    context.VSSetShader(&pipe.bars_vs, None);
                    context.PSSetShader(&pipe.bars_ps, None);
                    context.DrawInstanced(6, count, 0, 0);
                }
            }
            tex.key.bake_p0 = bake_p0;
            tex.key.price_to_px = view.price_to_px;
            tex.key.baked = true;
            tex.last_style = *style;
            tex.dirty = false;
            tex.last_bake_at = Some(now);
        }
    }

    /// Composite the cached order book into `view.bounds` of the caller-provided render target
    /// during an outer `BaseCache` rebuild.
    ///
    /// `panel_clip` is the panel scissor restored for layers that render afterward.
    pub fn render(
        &mut self,
        view: &ChartViewGpu,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
        panel_clip: [f32; 4],
    ) {
        let Some(tex) = self.tex.as_ref() else {
            return;
        };
        if !tex.key.baked || view.bounds[2] <= 0.0 || view.bounds[3] <= 0.0 {
            return;
        }
        // BLIT the ready texture's whole-texel window into the backbuffer order-book zone. Restore
        // `panel_clip` after the bake scissor or the following userdata layer would clip to this zone.
        let (uv_off, uv_scale) = book_blit_uv(&tex.key, view);
        let bp = BlitParams {
            dst: view.bounds,
            resolution: view.resolution,
            uv_off,
            uv_scale,
            pad: [0.0, 0.0],
        };
        update_dynamic(context, &tex.blit_cb, &[bp]);
        let vp = full_viewport(gpu);
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            context.RSSetViewports(Some(&[vp]));
            set_scissor_rect(
                context,
                panel_clip[0],
                panel_clip[1],
                panel_clip[2],
                panel_clip[3],
            );
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&tex.blit_vs, None);
            context.PSSetShader(&tex.blit_fs, None);
            context.VSSetConstantBuffers(0, Some(&[Some(tex.blit_cb.clone())]));
            context.PSSetConstantBuffers(0, Some(&[Some(tex.blit_cb.clone())]));
            context.PSSetShaderResources(0, Some(&[Some(tex.srv.clone())]));
            context.PSSetSamplers(0, Some(&[Some(tex.sampler.clone())]));
            context.OMSetBlendState(None, None, 0xFFFFFFFF);
            context.Draw(6, 0);
        }
    }

    fn create_pipe(device: &ID3D11Device, level_cap: u32) -> BookPipe {
        let bars_vs = make_vs(device, BARS_HLSL, "bars_vertex");
        let bars_ps = make_ps(device, BARS_HLSL, "bars_fragment");
        let bg_vs = make_vs(device, BARS_HLSL, "bg_vertex");
        let bg_ps = make_ps(device, BARS_HLSL, "bg_fragment");
        let blend = create_alpha_blend(device);
        let buffer = create_structured(
            device,
            std::mem::size_of::<LevelInstance>() as u32,
            level_cap.max(1),
        );
        let srv = create_srv(device, &buffer);
        let view_cb = create_dynamic_cb(device, std::mem::size_of::<ChartViewGpu>() as u32);
        let style_cb = create_dynamic_cb(device, std::mem::size_of::<BookStyle>() as u32);
        BookPipe {
            bars_vs,
            bars_ps,
            bg_vs,
            bg_ps,
            blend,
            buffer,
            srv,
            level_cap: level_cap.max(1),
            view_cb,
            style_cb,
        }
    }

    /// Creates an unbaked order-book texture with the supplied vertical margin.
    fn create_tex(device: &ID3D11Device, tex_w: u32, tex_h_total: u32, v_margin: f32) -> BookTex {
        let (tex, rtv, srv) = super::gpu::create_cache_texture(device, tex_w, tex_h_total);
        let blit_vs = make_vs(device, BLIT_HLSL, "blit_vertex");
        let blit_fs = make_ps(device, BLIT_HLSL, "blit_fragment");
        let blit_cb = create_dynamic_cb(device, std::mem::size_of::<BlitParams>() as u32);
        let sampler = create_point_sampler(device);
        BookTex {
            _tex: tex,
            rtv,
            srv,
            tex_w,
            blit_vs,
            blit_fs,
            blit_cb,
            sampler,
            key: BookBakeKey::unbaked(tex_h_total, v_margin),
            last_style: BookStyle::default(),
            dirty: false,
            last_bake_at: None,
        }
    }
}

fn next_buffer_cap(len: usize, floor: u32) -> u32 {
    (len as u32).max(1).max(floor).next_power_of_two()
}
