//! Combo layer for all immutable market history. It currently contains trades, with price lines
//! and volume sharing the layer. Crosses reside in a VRAM ring; a combo backing bitmap 20% wider
//! than the screen is baked by the cross shader and blitted with UV panning. Because historical
//! data is immutable, scrolling moves the baked bitmap and appending draws only the live edge
//! without redrawing history.
//!
//! Device-loss handling resets every resource when the hook's device generation changes after
//! GPUI recreates the device. Otherwise the new context would draw from stale buffers.

use bytemuck::Zeroable;
use gpui::RawGpuAccess;
use moon_core::data::PriceLinePoint;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::gpu::{
    BlitParams, ChartCross, ChartViewGpu, create_alpha_blend, create_dynamic_cb,
    create_point_sampler, create_premultiplied_alpha_blend, create_srv, create_structured,
    device_changed, full_viewport, ring_write_no_overwrite, set_scissor_rect, update_dynamic,
};
use super::types::{
    VolumeStats, append_cross_ring, cross_volume_stats, evicted_cross_ranges, ranges_have_entries,
    ranges_touch_volume_max, reset_cross_ring, subtract_cross_volume_stats,
    update_cross_volume_stats,
};

const MIN_COMBO_CAPACITY: u32 = 1;
const CROSSES_HLSL: &str = include_str!("shaders/crosses.hlsl");
const BLIT_HLSL: &str = include_str!("shaders/blit.hlsl");

#[inline]
fn texel_aligned_time0(time0: f32, time_to_px: f32) -> f32 {
    if !(time_to_px > 1e-9) {
        return time0;
    }
    (time0 * time_to_px).floor() / time_to_px
}

/// Cross-rendering pipeline and resident VRAM tick ring.
struct CrossPipe {
    cross_vs: ID3D11VertexShader,
    cross_ps: ID3D11PixelShader,
    volume_vs: ID3D11VertexShader,
    volume_ps: ID3D11PixelShader,
    price_vs: ID3D11VertexShader,
    price_last_ps: ID3D11PixelShader,
    price_mark_ps: ID3D11PixelShader,
    blend: ID3D11BlendState,
    premultiplied_blend: ID3D11BlendState,
    buffer: ID3D11Buffer,
    srv: ID3D11ShaderResourceView,
    last_line_buf: ID3D11Buffer,
    last_line_srv: ID3D11ShaderResourceView,
    mark_line_buf: ID3D11Buffer,
    mark_line_srv: ID3D11ShaderResourceView,
    view_cb: ID3D11Buffer,
}

/// Combo backing bitmap `(W * 1.2) x H`, containing baked history and a UV-scroll anchor.
struct ComboTex {
    _tex: ID3D11Texture2D, // Retain the texture through RAII while its RTV and SRV reference it.
    rtv: ID3D11RenderTargetView,
    srv: ID3D11ShaderResourceView,
    tex_w: u32,
    tex_h: u32,
    blit_vs: ID3D11VertexShader,
    blit_fs: ID3D11PixelShader,
    blit_cb: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    bake_t0: f32,
    last_baked_head: u32,
    last_time_to_px: f32,
    last_price_to_px: f32,
    last_view_price0: f32,
    last_marker_half: f32,
    last_volume_alpha: f32,
    last_volume_height_frac: f32,
    last_volume_style: f32,
    valid: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct VolumeScaleKey {
    data_generation: u64,
    bake_t0_bits: u32,
    tex_w_bits: u32,
    time_to_px_bits: u32,
}

pub struct ComboLayer {
    pipe: Option<CrossPipe>,
    tex: Option<ComboTex>,
    count: u32,
    head: u32,
    pending_reset: Option<Vec<ChartCross>>,
    pending_append: Vec<ChartCross>,
    pending_lines: Option<(Vec<PriceLinePoint>, Vec<PriceLinePoint>)>,
    resident_crosses: Vec<ChartCross>,
    resident_head: usize,
    resident_count: usize,
    last_line_count: u32,
    mark_line_count: u32,
    cross_capacity: u32,
    price_line_capacity: u32,
    /// `RawGpuAccess` device generation on which the resources were created; a change means loss.
    device_generation_seen: u64,
    /// Device generation incremented after each recreation. The orchestrator compares it with its
    /// last value and reuploads all history because appending the live edge cannot refill a new ring.
    device_gen: u64,
    volume_stats: VolumeStats,
    volume_scale_dirty: bool,
    volume_data_generation: u64,
    volume_window_cache: Option<(VolumeScaleKey, VolumeStats)>,
}

impl ComboLayer {
    pub fn new() -> Self {
        Self {
            pipe: None,
            tex: None,
            count: 0,
            head: 0,
            pending_reset: None,
            pending_append: Vec::new(),
            pending_lines: None,
            resident_crosses: Vec::new(),
            resident_head: 0,
            resident_count: 0,
            last_line_count: 0,
            mark_line_count: 0,
            cross_capacity: MIN_COMBO_CAPACITY,
            price_line_capacity: MIN_COMBO_CAPACITY,
            device_generation_seen: 0,
            device_gen: 0,
            volume_stats: VolumeStats::default(),
            volume_scale_dirty: false,
            volume_data_generation: 0,
            volume_window_cache: None,
        }
    }

    /// Combo device generation incremented on every device loss. The orchestrator compares it with
    /// `last_device_gen`; a change means the ring is empty and requires a full history reupload.
    pub fn device_gen(&self) -> u64 {
        self.device_gen
    }

    pub fn has_data(&self) -> bool {
        self.count > 0
    }

    pub fn volume_stats(&self) -> VolumeStats {
        self.volume_stats
    }

    pub fn set_capacity(&mut self, cross_capacity: usize, price_line_capacity: usize) {
        let cross_capacity = sanitize_capacity(cross_capacity);
        let price_line_capacity = sanitize_capacity(price_line_capacity);
        if self.cross_capacity == cross_capacity && self.price_line_capacity == price_line_capacity
        {
            return;
        }
        self.cross_capacity = cross_capacity;
        self.price_line_capacity = price_line_capacity;
        self.pipe = None;
        self.tex = None;
        self.count = 0;
        self.head = 0;
        self.resident_crosses.clear();
        self.resident_head = 0;
        self.resident_count = 0;
        self.last_line_count = 0;
        self.mark_line_count = 0;
        self.pending_append.clear();
        self.volume_data_generation = self.volume_data_generation.wrapping_add(1);
        self.volume_window_cache = None;
    }

    /// Reuploads the complete tick set after reloading market history and discards pending appends.
    pub fn reset(&mut self, data: Vec<ChartCross>) {
        self.pending_reset = Some(data);
        self.pending_append.clear();
    }

    /// Appends newly arrived ticks to the ring's live edge.
    pub fn append(&mut self, data: &[ChartCross]) {
        if !data.is_empty() {
            self.pending_append.extend_from_slice(data);
        }
    }

    pub fn set_price_lines(&mut self, last: &[PriceLinePoint], mark: &[PriceLinePoint]) {
        self.pending_lines = Some((last.to_vec(), mark.to_vec()));
    }

    /// Prepare phase: uploads pending data and bakes/extends the offscreen combo texture.
    /// This may switch render targets and must run from `GpuCanvasDriver::prepare_gpu`.
    pub fn prepare(
        &mut self,
        view: &ChartViewGpu,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        gpu: &RawGpuAccess,
    ) {
        // A new device invalidates the old buffers, shaders, and ring. Reset both resources and ring
        // counters because the recreated buffer is empty and a stale count would make DrawInstanced
        // read garbage. Incrementing device_gen makes prepare reupload all history via collect_all.
        if device_changed(&mut self.device_generation_seen, gpu) {
            self.pipe = None;
            self.tex = None;
            self.count = 0;
            self.head = 0;
            self.resident_crosses.clear();
            self.resident_head = 0;
            self.resident_count = 0;
            self.last_line_count = 0;
            self.mark_line_count = 0;
            self.volume_data_generation = self.volume_data_generation.wrapping_add(1);
            self.volume_window_cache = None;
            self.device_gen = self.device_gen.wrapping_add(1);
        }
        if self.pipe.is_none() {
            self.pipe = Some(self.create_pipe(device));
        }
        self.apply_uploads(context);
        if self.volume_scale_dirty {
            if let Some(tex) = self.tex.as_mut() {
                tex.valid = false;
            }
            self.volume_scale_dirty = false;
        }
        if self.count == 0 {
            return;
        }
        self.prepare_combo(view, device, context);
    }

    /// Draws combo into the hook backbuffer during `UnderScene` after `prepare()` uploads and bakes.
    pub fn render(
        &mut self,
        view: &ChartViewGpu,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
        panel_clip: [f32; 4],
    ) {
        if self.count == 0 && self.last_line_count <= 1 && self.mark_line_count <= 1 {
            return;
        }
        if self.count > 0 {
            self.blit_combo(view, context, rtv, gpu, panel_clip);
        }
        self.draw_price_lines_to_backbuffer(view, context, rtv, gpu, panel_clip);
    }

    /// Incrementally bakes new combo ticks into the texture. Performs a full rebake when the 20%
    /// margin is exhausted or the bitmap is invalid due to zoom, resize, or the first frame.
    fn prepare_combo(
        &mut self,
        view: &ChartViewGpu,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
    ) {
        let bw = view.bounds[2];
        let bh = view.bounds[3];
        if bw <= 0.0 || bh <= 0.0 {
            return;
        }
        let margin_px = (bw * 0.2).max(128.0);
        let tex_w = (bw + margin_px).round().max(1.0) as u32;
        let tex_h = bh.round().max(1.0) as u32;
        let need_new = self
            .tex
            .as_ref()
            .map_or(true, |c| c.tex_w != tex_w || c.tex_h != tex_h);
        if need_new {
            self.tex = Some(Self::create_tex(device, tex_w, tex_h));
        }
        let ttp = view.time_to_px;
        let tex_ref = self.tex.as_ref().unwrap();
        let transform_changed = tex_ref.last_time_to_px != view.time_to_px
            || tex_ref.last_price_to_px != view.price_to_px
            || tex_ref.last_view_price0 != view.view_price0
            || tex_ref.last_marker_half != view.marker_half
            || tex_ref.last_volume_alpha != view.volume_alpha
            || tex_ref.last_volume_height_frac != view.volume_height_frac
            || tex_ref.last_volume_style != view.volume_style;
        let u_left_px = (view.view_time0 - tex_ref.bake_t0) * ttp;
        let mut need_full =
            transform_changed || !tex_ref.valid || u_left_px < 0.0 || u_left_px > margin_px;
        let bake_t0 = if need_full {
            texel_aligned_time0(view.view_time0, ttp)
        } else {
            tex_ref.bake_t0
        };
        let window_stats = self.volume_stats_for_bake_window(bake_t0, tex_w as f32, ttp);
        let pipe = self.pipe.as_ref().unwrap();
        let tex = self.tex.as_mut().unwrap();
        if transform_changed {
            tex.valid = false;
        }
        if window_stats.scale() != self.volume_stats.scale() {
            self.volume_stats = window_stats;
            tex.valid = false;
            need_full = true;
        }
        // The bake uniform fixes the left time edge at bake_t0 and covers the full bitmap viewport.
        // During a full rebake, keep bake_t0 on the global texel phase. Otherwise the old bake plus
        // rounded UV scroll can differ from the new raw view_time0 by one pixel, making historical
        // crosses visibly jump when the margin is exhausted.
        let bake_view = ChartViewGpu {
            bounds: [0.0, 0.0, tex_w as f32, tex_h as f32],
            resolution: [tex_w as f32, tex_h as f32],
            time_to_px: ttp,
            view_time0: bake_t0,
            price_to_px: view.price_to_px,
            view_price0: view.view_price0,
            marker_half: view.marker_half,
            // crosses.hlsl: combo-pass first-instance offset into the resident ring buffer.
            pad: 0.0,
            volume_buy_inv: 1.0 / self.volume_stats.buy_max.max(1e-6),
            volume_sell_inv: 1.0 / self.volume_stats.sell_max.max(1e-6),
            volume_alpha: view.volume_alpha,
            volume_height_frac: view.volume_height_frac,
            price_line: view.price_line,
            mark_price_line: view.mark_price_line,
            price_line_width: view.price_line_width,
            volume_style: view.volume_style,
            _pad3: [0.0; 2],
        };
        update_dynamic(context, &pipe.view_cb, &[bake_view]);
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
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetConstantBuffers(0, Some(&[Some(pipe.view_cb.clone())]));
            context.PSSetConstantBuffers(0, Some(&[Some(pipe.view_cb.clone())]));
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            if need_full {
                crate::diag::bump(&crate::diag::CHART_COMBO_BAKE);
                tex.bake_t0 = bake_t0;
                // Keep the bitmap background transparent so only crosses are opaque. Alpha blitting
                // then reveals the lower grid/background layer between crosses; grid applies #131416.
                context.ClearRenderTargetView(&tex.rtv, &[0.0, 0.0, 0.0, 0.0]);
                context.VSSetShaderResources(1, Some(&[Some(pipe.srv.clone())]));
                context.VSSetShader(&pipe.volume_vs, None);
                context.PSSetShader(&pipe.volume_ps, None);
                context.DrawInstanced(6, self.count, 0, 0);
                context.VSSetShader(&pipe.cross_vs, None);
                context.PSSetShader(&pipe.cross_ps, None);
                context.DrawInstanced(6, self.count, 0, 0);
                tex.last_baked_head = self.head;
                tex.last_time_to_px = view.time_to_px;
                tex.last_price_to_px = view.price_to_px;
                tex.last_view_price0 = view.view_price0;
                tex.last_marker_half = view.marker_half;
                tex.last_volume_alpha = view.volume_alpha;
                tex.last_volume_height_frac = view.volume_height_frac;
                tex.last_volume_style = view.volume_style;
                tex.valid = true;
            } else if self.head != tex.last_baked_head {
                // Incrementally draw only new ring ticks in [last_head, head), including wraparound.
                let cap = self.cross_capacity;
                let delta = (self.head + cap - tex.last_baked_head) % cap;
                let runs: [(u32, u32); 2] = if tex.last_baked_head + delta <= cap {
                    [(tex.last_baked_head, delta), (0, 0)]
                } else {
                    [
                        (tex.last_baked_head, cap - tex.last_baked_head),
                        (0, delta - (cap - tex.last_baked_head)),
                    ]
                };
                for (rf, rc) in runs {
                    if rc == 0 {
                        continue;
                    }
                    let mut run_view = bake_view;
                    run_view.pad = rf as f32;
                    update_dynamic(context, &pipe.view_cb, &[run_view]);
                    context.VSSetShaderResources(1, Some(&[Some(pipe.srv.clone())]));
                    context.VSSetShader(&pipe.volume_vs, None);
                    context.PSSetShader(&pipe.volume_ps, None);
                    context.DrawInstanced(6, rc, 0, 0);
                    context.VSSetShader(&pipe.cross_vs, None);
                    context.PSSetShader(&pipe.cross_ps, None);
                    context.DrawInstanced(6, rc, 0, 0);
                }
                tex.last_baked_head = self.head;
            }
            super::gpu::debug_dump_combo_texture_once(device, context, &tex._tex);
        }
    }

    fn blit_combo(
        &mut self,
        view: &ChartViewGpu,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
        panel_clip: [f32; 4],
    ) {
        let bw = view.bounds[2];
        let Some(pipe) = self.pipe.as_ref() else {
            return;
        };
        let Some(tex) = self.tex.as_mut() else {
            return;
        };
        if !tex.valid || bw <= 0.0 {
            return;
        }
        // Composite the visible bitmap window into the backbuffer chart area with point sampling.
        // Keep the UV offset in whole texels because a fractional point-sampled offset causes
        // half-pixel flicker during live scrolling.
        let u_left_px = (view.view_time0 - tex.bake_t0) * view.time_to_px;
        let tex_w = tex.tex_w;
        let u_left_px = u_left_px.round().clamp(0.0, (tex_w as f32 - bw).max(0.0));
        let u_left = u_left_px / tex_w as f32;
        let u_span = bw / tex_w as f32;
        let bp = BlitParams {
            dst: view.bounds,
            resolution: view.resolution,
            uv_off: [u_left, 0.0],
            uv_scale: [u_span, 1.0],
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
            context.OMSetBlendState(&pipe.premultiplied_blend, None, 0xFFFFFFFF);
            context.Draw(6, 0);
        }
    }

    fn draw_price_lines_to_backbuffer(
        &self,
        view: &ChartViewGpu,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
        panel_clip: [f32; 4],
    ) {
        if self.last_line_count <= 1 && self.mark_line_count <= 1 {
            return;
        }
        let Some(pipe) = self.pipe.as_ref() else {
            return;
        };
        update_dynamic(context, &pipe.view_cb, std::slice::from_ref(view));
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
            context.VSSetConstantBuffers(0, Some(&[Some(pipe.view_cb.clone())]));
            context.PSSetConstantBuffers(0, Some(&[Some(pipe.view_cb.clone())]));
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            Self::draw_price_lines(context, pipe, self.last_line_count, self.mark_line_count);
        }
    }

    fn apply_uploads(&mut self, context: &ID3D11DeviceContext) {
        let (tick_buffer, last_line_buf, mark_line_buf) = {
            let pipe = self.pipe.as_ref().unwrap();
            (
                pipe.buffer.clone(),
                pipe.last_line_buf.clone(),
                pipe.mark_line_buf.clone(),
            )
        };
        if let Some((last, mark)) = self.pending_lines.take() {
            self.last_line_count =
                upload_points(context, &last_line_buf, &last, self.price_line_capacity);
            self.mark_line_count =
                upload_points(context, &mark_line_buf, &mark, self.price_line_capacity);
        }
        if let Some(data) = self.pending_reset.take() {
            // On overflow, retain only the most recent capacity-sized tail.
            let cap = self.cross_capacity;
            let data: &[ChartCross] = if data.len() as u32 > cap {
                &data[data.len() - cap as usize..]
            } else {
                &data
            };
            update_dynamic(context, &tick_buffer, data);
            self.count = data.len() as u32;
            self.head = (data.len() as u32) % cap;
            reset_cross_ring(
                &mut self.resident_crosses,
                &mut self.resident_head,
                &mut self.resident_count,
                cap as usize,
                data,
            );
            if self.resident_crosses.len() < cap as usize {
                self.resident_crosses
                    .resize(cap as usize, ChartCross::zeroed());
            }
            self.recalc_volume_scale();
            self.volume_scale_dirty = true;
            self.volume_data_generation = self.volume_data_generation.wrapping_add(1);
            self.volume_window_cache = None;
        }
        if !self.pending_append.is_empty() {
            let data = std::mem::take(&mut self.pending_append);
            let cap = self.cross_capacity;
            let data: &[ChartCross] = if data.len() as u32 > cap {
                &data[data.len() - cap as usize..]
            } else {
                &data
            };
            let before_scale = self.volume_stats.scale();
            let old_head = self.resident_head;
            let old_count = self.resident_count;
            let full_reset = data.len() >= cap as usize;
            let evicted_ranges =
                evicted_cross_ranges(old_head, old_count, cap as usize, data.len());
            let evicted_any = ranges_have_entries(&evicted_ranges);
            let evicted_scale_max =
                ranges_touch_volume_max(&self.resident_crosses, &evicted_ranges, before_scale);
            let mut next_stats = self.volume_stats;
            subtract_cross_volume_stats(&mut next_stats, &self.resident_crosses, &evicted_ranges);
            let n = data.len() as u32;
            ring_write_no_overwrite(context, &tick_buffer, self.head, cap, data);
            self.head = (self.head + n) % cap;
            self.count = (self.count + n).min(cap);
            append_cross_ring(
                &mut self.resident_crosses,
                &mut self.resident_head,
                &mut self.resident_count,
                cap as usize,
                data,
            );
            if self.resident_crosses.len() < cap as usize {
                self.resident_crosses
                    .resize(cap as usize, ChartCross::zeroed());
            }
            if full_reset || evicted_scale_max {
                self.recalc_volume_scale();
            } else {
                self.update_volume_scale(&mut next_stats, data);
            }
            if before_scale != self.volume_stats.scale() {
                self.volume_scale_dirty = true;
            }
            self.volume_data_generation = self.volume_data_generation.wrapping_add(1);
            self.volume_window_cache = None;
            if full_reset || evicted_any {
                if let Some(tex) = self.tex.as_mut() {
                    tex.valid = false;
                }
            }
        }
    }

    fn recalc_volume_scale(&mut self) {
        self.volume_stats =
            cross_volume_stats(self.resident_crosses.iter().take(self.resident_count));
    }

    fn update_volume_scale(&mut self, stats: &mut VolumeStats, data: &[ChartCross]) {
        update_cross_volume_stats(stats, data);
        self.volume_stats = *stats;
    }

    fn volume_stats_for_bake_window(
        &mut self,
        bake_t0: f32,
        tex_w: f32,
        time_to_px: f32,
    ) -> VolumeStats {
        if !(time_to_px > 1e-9) || self.resident_count == 0 {
            return VolumeStats::default();
        }
        let key = VolumeScaleKey {
            data_generation: self.volume_data_generation,
            bake_t0_bits: bake_t0.to_bits(),
            tex_w_bits: tex_w.to_bits(),
            time_to_px_bits: time_to_px.to_bits(),
        };
        if let Some((cached_key, cached)) = self.volume_window_cache {
            if cached_key == key {
                return cached;
            }
        }
        let time_left = bake_t0 - 2.0 / time_to_px;
        let time_right = bake_t0 + (tex_w + 2.0) / time_to_px;
        let capacity = self.cross_capacity.max(1) as usize;
        let count = self
            .resident_count
            .min(capacity)
            .min(self.resident_crosses.len());
        let start = if count == capacity {
            self.resident_head % capacity
        } else {
            0
        };
        let mut stats = VolumeStats::default();
        for i in 0..count {
            let idx = (start + i) % capacity;
            let Some(c) = self.resident_crosses.get(idx) else {
                continue;
            };
            if c.time_rel < time_left || c.time_rel > time_right || c.qty <= 0.0 {
                continue;
            }
            update_cross_volume_stats(&mut stats, std::slice::from_ref(c));
        }
        let out = stats;
        self.volume_window_cache = Some((key, out));
        out
    }

    fn draw_price_lines(
        context: &ID3D11DeviceContext,
        pipe: &CrossPipe,
        last_line_count: u32,
        mark_line_count: u32,
    ) {
        unsafe {
            context.VSSetShader(&pipe.price_vs, None);
            if last_line_count > 1 {
                context.VSSetShaderResources(2, Some(&[Some(pipe.last_line_srv.clone())]));
                context.PSSetShader(&pipe.price_last_ps, None);
                context.DrawInstanced(6, last_line_count - 1, 0, 0);
            }
            if mark_line_count > 1 {
                context.VSSetShaderResources(2, Some(&[Some(pipe.mark_line_srv.clone())]));
                context.PSSetShader(&pipe.price_mark_ps, None);
                context.DrawInstanced(6, mark_line_count - 1, 0, 0);
            }
        }
    }

    fn create_pipe(&self, device: &ID3D11Device) -> CrossPipe {
        let cross_vs = super::gpu::make_vs(device, CROSSES_HLSL, "crosses_vertex");
        let cross_ps = super::gpu::make_ps(device, CROSSES_HLSL, "crosses_fragment");
        let volume_vs = super::gpu::make_vs(device, CROSSES_HLSL, "volume_vertex");
        let volume_ps = super::gpu::make_ps(device, CROSSES_HLSL, "volume_fragment");
        let price_vs = super::gpu::make_vs(device, CROSSES_HLSL, "price_line_vertex");
        let price_last_ps = super::gpu::make_ps(device, CROSSES_HLSL, "price_last_fragment");
        let price_mark_ps = super::gpu::make_ps(device, CROSSES_HLSL, "price_mark_fragment");
        let blend = create_alpha_blend(device);
        let premultiplied_blend = create_premultiplied_alpha_blend(device);
        let buffer = create_structured(
            device,
            std::mem::size_of::<ChartCross>() as u32,
            self.cross_capacity,
        );
        let srv = create_srv(device, &buffer);
        let last_line_buf = create_structured(
            device,
            std::mem::size_of::<PriceLinePoint>() as u32,
            self.price_line_capacity,
        );
        let last_line_srv = create_srv(device, &last_line_buf);
        let mark_line_buf = create_structured(
            device,
            std::mem::size_of::<PriceLinePoint>() as u32,
            self.price_line_capacity,
        );
        let mark_line_srv = create_srv(device, &mark_line_buf);
        let view_cb = create_dynamic_cb(device, std::mem::size_of::<ChartViewGpu>() as u32);
        CrossPipe {
            cross_vs,
            cross_ps,
            volume_vs,
            volume_ps,
            price_vs,
            price_last_ps,
            price_mark_ps,
            blend,
            premultiplied_blend,
            buffer,
            srv,
            last_line_buf,
            last_line_srv,
            mark_line_buf,
            mark_line_srv,
            view_cb,
        }
    }

    fn create_tex(device: &ID3D11Device, tex_w: u32, tex_h: u32) -> ComboTex {
        let (tex, rtv, srv) = super::gpu::create_cache_texture(device, tex_w, tex_h);
        let blit_vs = super::gpu::make_vs(device, BLIT_HLSL, "blit_vertex");
        let blit_fs = super::gpu::make_ps(device, BLIT_HLSL, "blit_fragment");
        let blit_cb = create_dynamic_cb(device, std::mem::size_of::<BlitParams>() as u32);
        let sampler = create_point_sampler(device);
        ComboTex {
            _tex: tex,
            rtv,
            srv,
            tex_w,
            tex_h,
            blit_vs,
            blit_fs,
            blit_cb,
            sampler,
            bake_t0: 0.0,
            last_baked_head: u32::MAX,
            last_time_to_px: f32::NAN,
            last_price_to_px: f32::NAN,
            last_view_price0: f32::NAN,
            last_marker_half: f32::NAN,
            last_volume_alpha: f32::NAN,
            last_volume_height_frac: f32::NAN,
            last_volume_style: f32::NAN,
            valid: false,
        }
    }
}

fn upload_points(
    context: &ID3D11DeviceContext,
    buffer: &ID3D11Buffer,
    data: &[PriceLinePoint],
    cap: u32,
) -> u32 {
    let data = if data.len() as u32 > cap {
        &data[data.len() - cap as usize..]
    } else {
        data
    };
    if !data.is_empty() {
        update_dynamic(context, buffer, data);
    }
    data.len() as u32
}

fn sanitize_capacity(capacity: usize) -> u32 {
    capacity.clamp(MIN_COMBO_CAPACITY as usize, u32::MAX as usize) as u32
}
