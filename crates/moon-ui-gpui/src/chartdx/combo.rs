//! Combo layer for all immutable market history: trade crosses, their volume bars and price
//! lines. Crosses reside in a VRAM ring; the cross shader bakes them into a bitmap wider and taller
//! than the chart, and the volume bars into a separate band bitmap with no price axis. Both are
//! blitted with UV panning, so scrolling on either axis moves a baked bitmap and appending draws
//! only the live edge without redrawing history.
//!
//! Device-loss handling resets every resource when the hook's device generation changes after
//! GPUI recreates the device. Otherwise the new context would draw from stale buffers.

use bytemuck::Zeroable;
use gpui::RawGpuAccess;
use moon_chart::tick_volume::{
    BakeColumns, LodPick, TickTimeOrder, lod_applies, pending_ring_at, reduce_crosses,
    reduce_volume, tick_bake_span, tick_slot_runs, tick_time_range, tick_touches_bake,
};
use moon_core::data::PriceLinePoint;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::gpu::{
    BlitParams, ChartCross, ChartViewGpu, create_alpha_blend, create_dynamic_cb,
    create_point_sampler, create_premultiplied_alpha_blend, create_srv, create_structured,
    device_changed, full_viewport, ring_write_no_overwrite, set_scissor_rect, update_dynamic,
};
use super::types::{
    PriceStyleGpu, TickStyleGpu, append_cross_ring, evicted_cross_ranges, reset_cross_ring,
};

const MIN_COMBO_CAPACITY: u32 = 1;
const CROSSES_HLSL: &str = include_str!("shaders/crosses.hlsl");
const BLIT_HLSL: &str = include_str!("shaders/blit.hlsl");

mod plan;

use plan::{
    ComboBakeKey, VolumeBakeKey, combo_v_margin_px, combo_x_margin_px, cross_blit_uv,
    lod_instance_count, plan_cross_bake, plan_volume_bake, volume_band_px, volume_blit_uv,
};

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
    /// Rows a dense full bake keeps after LOD reduction, grown in powers of two.
    lod_buffer: ID3D11Buffer,
    lod_srv: ID3D11ShaderResourceView,
    lod_capacity: u32,
    last_line_buf: ID3D11Buffer,
    last_line_srv: ID3D11ShaderResourceView,
    mark_line_buf: ID3D11Buffer,
    mark_line_srv: ID3D11ShaderResourceView,
    view_cb: ID3D11Buffer,
    price_style_cb: ID3D11Buffer,
    tick_style_cb: ID3D11Buffer,
    /// Blit resources shared by the cross and volume bitmaps.
    blit_vs: ID3D11VertexShader,
    blit_fs: ID3D11PixelShader,
    blit_cb: ID3D11Buffer,
    sampler: ID3D11SamplerState,
}

/// Cross bitmap `(W * 1.2) x (H + 2 * margin)`, containing baked crosses and a UV-scroll anchor.
struct ComboTex {
    _tex: ID3D11Texture2D, // Retain the texture through RAII while its RTV and SRV reference it.
    rtv: ID3D11RenderTargetView,
    srv: ID3D11ShaderResourceView,
    key: ComboBakeKey,
    last_baked_head: u32,
}

/// Volume band bitmap `(W * 1.2) x band`, baked without a price axis so Y motion never touches it.
struct VolumeTex {
    _tex: ID3D11Texture2D, // Retain the texture through RAII while its RTV and SRV reference it.
    rtv: ID3D11RenderTargetView,
    srv: ID3D11ShaderResourceView,
    key: VolumeBakeKey,
    last_baked_head: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct VolumeScaleKey {
    data_generation: u64,
    bake_t0_bits: u32,
    tex_w_bits: u32,
    time_to_px_bits: u32,
}

/// Retains tick history and appearance for cached combo rendering.
pub struct ComboLayer {
    pipe: Option<CrossPipe>,
    tex: Option<ComboTex>,
    vol_tex: Option<VolumeTex>,
    count: u32,
    head: u32,
    pending_reset: Option<Vec<ChartCross>>,
    pending_append: Vec<ChartCross>,
    pending_lines: Option<(Vec<PriceLinePoint>, Vec<PriceLinePoint>)>,
    resident_crosses: Vec<ChartCross>,
    resident_head: usize,
    resident_count: usize,
    /// Ordering evidence for resident and queued rows, maintained only when data arrives.
    tick_time_order: TickTimeOrder,
    last_line_count: u32,
    mark_line_count: u32,
    cross_capacity: u32,
    price_line_capacity: u32,
    /// `RawGpuAccess` device generation on which the resources were created; a change means loss.
    device_generation_seen: u64,
    /// Device generation incremented after each recreation. The orchestrator compares it with its
    /// last value and reuploads all history because appending the live edge cannot refill a new ring.
    device_gen: u64,
    volume_buy_max: f32,
    volume_sell_max: f32,
    volume_scale_dirty: bool,
    volume_data_generation: u64,
    volume_window_cache: Option<(VolumeScaleKey, (f32, f32))>,
    /// Price-line appearance uploaded with the view uniform on the main pass.
    ///
    /// Not part of the combo bake: price lines are drawn straight to the backbuffer, so a
    /// change here needs no texture invalidation the way `volume_alpha` does.
    price_style: PriceStyleGpu,
    /// Trade-tick style retained across resource recreation and compared before rebaking.
    tick_style: TickStyleGpu,
    /// Reusable LOD reduction scratch and the gathered rows it uploads.
    lod_pick: LodPick,
    lod_rows: Vec<ChartCross>,
}

impl ComboLayer {
    /// Creates empty GPU resources with retained default appearance.
    pub fn new() -> Self {
        Self {
            pipe: None,
            tex: None,
            vol_tex: None,
            count: 0,
            head: 0,
            pending_reset: None,
            pending_append: Vec::new(),
            pending_lines: None,
            resident_crosses: Vec::new(),
            resident_head: 0,
            resident_count: 0,
            tick_time_order: TickTimeOrder::default(),
            last_line_count: 0,
            mark_line_count: 0,
            cross_capacity: MIN_COMBO_CAPACITY,
            price_line_capacity: MIN_COMBO_CAPACITY,
            device_generation_seen: 0,
            device_gen: 0,
            volume_buy_max: 1e-6,
            volume_sell_max: 1e-6,
            volume_scale_dirty: false,
            volume_data_generation: 0,
            volume_window_cache: None,
            price_style: PriceStyleGpu::default(),
            tick_style: TickStyleGpu::default(),
            lod_pick: LodPick::default(),
            lod_rows: Vec::new(),
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

    /// Resize GPU storage and retire ordering evidence with the resident ring.
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
        self.vol_tex = None;
        self.count = 0;
        self.head = 0;
        self.resident_crosses.clear();
        self.resident_head = 0;
        self.resident_count = 0;
        self.last_line_count = 0;
        self.mark_line_count = 0;
        self.pending_append.clear();
        self.refresh_pending_time_order();
        self.volume_data_generation = self.volume_data_generation.wrapping_add(1);
        self.volume_window_cache = None;
    }

    /// Rebuild ordering evidence after resident storage is retired, keeping queued rows visible.
    fn refresh_pending_time_order(&mut self) {
        self.tick_time_order = TickTimeOrder::default();
        self.tick_time_order.extend(
            moon_chart::tick_volume::pending_ring(
                &self.resident_crosses,
                self.resident_head,
                self.resident_count,
                self.cross_capacity as usize,
                self.pending_reset.as_deref(),
                &self.pending_append,
            )
            .map(|c| c.time_rel),
        );
    }

    /// Reuploads the complete tick set after reloading market history and discards pending appends.
    pub fn reset(&mut self, data: Vec<ChartCross>) {
        self.tick_time_order = TickTimeOrder::default();
        self.tick_time_order.extend(
            data.iter()
                .skip(data.len().saturating_sub(self.cross_capacity as usize))
                .map(|c| c.time_rel),
        );
        self.pending_reset = Some(data);
        self.pending_append.clear();
    }

    /// Append live ticks and retain lateness evidence even when this batch replaces the ring.
    pub fn append(&mut self, data: &[ChartCross]) {
        if !data.is_empty() {
            self.tick_time_order.extend(data.iter().map(|c| c.time_rel));
            self.pending_append.extend_from_slice(data);
        }
    }

    /// Updates tick colours and invalidates history baked with the previous style.
    pub fn set_tick_style(&mut self, style: TickStyleGpu) {
        if self.tick_style != style {
            self.tick_style = style;
            self.invalidate_bakes();
        }
    }

    /// Idempotently sets the price-line colours and half-width.
    pub fn set_price_style(&mut self, style: PriceStyleGpu) {
        self.price_style = style;
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
            self.vol_tex = None;
            self.count = 0;
            self.head = 0;
            self.resident_crosses.clear();
            self.resident_head = 0;
            self.resident_count = 0;
            self.refresh_pending_time_order();
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
            self.invalidate_bakes();
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

    /// Force the next prepare to fully rebake both bitmaps.
    fn invalidate_bakes(&mut self) {
        if let Some(tex) = self.tex.as_mut() {
            tex.key.valid = false;
        }
        if let Some(tex) = self.vol_tex.as_mut() {
            tex.key.valid = false;
        }
    }

    /// Bakes new ticks into the cross and volume bitmaps. Each planner decides a full rebake when
    /// its baked window no longer covers the view; otherwise only newly appended ring rows draw.
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
        let tex_w = (bw + combo_x_margin_px(bw)).round().max(1.0) as u32;
        let tex_h = bh.round().max(1.0) as u32;
        let v_margin = combo_v_margin_px(bh);
        let tex_h_total = tex_h + 2 * v_margin as u32;
        let band_px = volume_band_px(bh);
        if self.tex.as_ref().map_or(true, |c| {
            c.key.tex_w != tex_w || c.key.tex_h_total != tex_h_total
        }) {
            self.tex = Some(Self::create_tex(device, tex_w, tex_h_total, v_margin));
        }
        if self.vol_tex.as_ref().map_or(true, |c| {
            c.key.tex_w != tex_w || c.key.band_px != band_px || c.key.chart_h != tex_h
        }) {
            self.vol_tex = Some(Self::create_vol_tex(device, tex_w, band_px, tex_h));
        }
        let ttp = view.time_to_px;
        let cross_plan = plan_cross_bake(&self.tex.as_ref().unwrap().key, view, bw);
        let vol_key = self.vol_tex.as_ref().unwrap().key;
        let vol_plan = plan_volume_bake(&vol_key, view, bw, |bake_t0| {
            self.volume_scale_for_bake_window(bake_t0, tex_w as f32, ttp)
        });
        (self.volume_buy_max, self.volume_sell_max) = vol_plan.scale;
        let runs_from = |layer: &Self, bake_t0: f32, marker_half: f32| {
            let span = tick_bake_span(bake_t0, tex_w as f32, ttp, marker_half);
            tick_slot_runs(
                layer.resident_time_range(span.0, span.1),
                layer.resident_head,
                layer.resident_count,
                layer.cross_capacity as usize,
            )
        };
        let cross_runs = runs_from(self, cross_plan.bake_t0, view.marker_half);
        let vol_runs = runs_from(self, vol_plan.bake_t0, 0.0);
        let cross_view = ChartViewGpu {
            bounds: [0.0, 0.0, tex_w as f32, tex_h_total as f32],
            resolution: [tex_w as f32, tex_h_total as f32],
            time_to_px: ttp,
            view_time0: cross_plan.bake_t0,
            price_to_px: view.price_to_px,
            view_price0: cross_plan.bake_p0,
            marker_half: view.marker_half,
            // crosses.hlsl: combo-pass first-instance offset into the resident ring buffer.
            pad: 0.0,
            volume_buy_inv: 1.0 / self.volume_buy_max.max(1e-6),
            volume_sell_inv: 1.0 / self.volume_sell_max.max(1e-6),
            volume_alpha: view.volume_alpha,
            _pad2: 0.0,
        };
        // The band keeps the chart's full height as its bounds so bar heights match a direct draw;
        // only the bottom `band_px` rows land inside the target.
        let vol_view = ChartViewGpu {
            bounds: [
                0.0,
                band_px as f32 - tex_h as f32,
                tex_w as f32,
                tex_h as f32,
            ],
            resolution: [tex_w as f32, band_px as f32],
            view_time0: vol_plan.bake_t0,
            view_price0: view.view_price0,
            ..cross_view
        };
        let head = self.head;
        let cap = self.cross_capacity;
        update_dynamic(
            context,
            &self.pipe.as_ref().unwrap().tick_style_cb,
            &[self.tick_style],
        );
        if vol_plan.full {
            crate::diag::bump(&crate::diag::CHART_COMBO_VOLUME_BAKE);
            let cols = BakeColumns {
                time0: vol_plan.bake_t0,
                time_to_px: ttp,
                price0: view.view_price0,
                price_to_px: view.price_to_px,
                height: tex_h as f32,
                width_px: tex_w,
                volume_alpha: view.volume_alpha,
                marker_half: view.marker_half,
                buy_inv: 1.0 / self.volume_buy_max.max(1e-6),
                sell_inv: 1.0 / self.volume_sell_max.max(1e-6),
            };
            let lod = self.upload_lod(device, context, vol_runs, &cols, false);
            let pipe = self.pipe.as_ref().unwrap();
            let vol = self.vol_tex.as_mut().unwrap();
            let (srv, runs) = match lod {
                Some(n) => (&pipe.lod_srv, [(0, n), (0, 0)]),
                None => (&pipe.srv, slot_runs_u32(vol_runs)),
            };
            Self::bake_pass(
                context,
                pipe,
                &vol.rtv,
                vol_view,
                (&pipe.volume_vs, &pipe.volume_ps),
                srv,
                runs,
                true,
            );
            vol.key.bake_t0 = vol_plan.bake_t0;
            vol.key.time_to_px = ttp;
            vol.key.volume_alpha = view.volume_alpha;
            vol.key.scale = vol_plan.scale;
            vol.key.valid = true;
            vol.last_baked_head = head;
        } else {
            let pipe = self.pipe.as_ref().unwrap();
            let vol = self.vol_tex.as_mut().unwrap();
            if head != vol.last_baked_head {
                Self::bake_pass(
                    context,
                    pipe,
                    &vol.rtv,
                    vol_view,
                    (&pipe.volume_vs, &pipe.volume_ps),
                    &pipe.srv,
                    appended_runs(vol.last_baked_head, head, cap),
                    false,
                );
                vol.last_baked_head = head;
            }
        }
        if cross_plan.full {
            crate::diag::bump(&crate::diag::CHART_COMBO_BAKE);
            let cols = BakeColumns {
                time0: cross_plan.bake_t0,
                time_to_px: ttp,
                price0: cross_plan.bake_p0,
                price_to_px: view.price_to_px,
                height: tex_h_total as f32,
                width_px: tex_w,
                volume_alpha: view.volume_alpha,
                marker_half: view.marker_half,
                buy_inv: 1.0 / self.volume_buy_max.max(1e-6),
                sell_inv: 1.0 / self.volume_sell_max.max(1e-6),
            };
            let lod = self.upload_lod(device, context, cross_runs, &cols, true);
            let pipe = self.pipe.as_ref().unwrap();
            let tex = self.tex.as_mut().unwrap();
            let (srv, runs) = match lod {
                Some(n) => (&pipe.lod_srv, [(0, n), (0, 0)]),
                None => (&pipe.srv, slot_runs_u32(cross_runs)),
            };
            crate::diag::bump_by(
                &crate::diag::CHART_COMBO_BAKE_INSTANCES,
                runs.iter().map(|&(_, count)| u64::from(count)).sum(),
            );
            Self::bake_pass(
                context,
                pipe,
                &tex.rtv,
                cross_view,
                (&pipe.cross_vs, &pipe.cross_ps),
                srv,
                runs,
                true,
            );
            tex.key.bake_t0 = cross_plan.bake_t0;
            tex.key.bake_p0 = cross_plan.bake_p0;
            tex.key.time_to_px = ttp;
            tex.key.price_to_px = view.price_to_px;
            tex.key.marker_half = view.marker_half;
            tex.key.valid = true;
            tex.last_baked_head = head;
        } else {
            let pipe = self.pipe.as_ref().unwrap();
            let tex = self.tex.as_mut().unwrap();
            if head != tex.last_baked_head {
                Self::bake_pass(
                    context,
                    pipe,
                    &tex.rtv,
                    cross_view,
                    (&pipe.cross_vs, &pipe.cross_ps),
                    &pipe.srv,
                    appended_runs(tex.last_baked_head, head, cap),
                    false,
                );
                tex.last_baked_head = head;
            }
        }
        let tex = self.tex.as_ref().unwrap();
        super::gpu::debug_dump_combo_texture_once(device, context, &tex._tex);
    }

    /// Thin a dense full bake to what its bitmap can show and upload the picked rows, in draw
    /// order, to the LOD buffer; `None` keeps the raw ring draw.
    fn upload_lod(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        runs: [(usize, usize); 2],
        cols: &BakeColumns,
        crosses: bool,
    ) -> Option<u32> {
        let rows_in_span = runs.iter().map(|&(_, count)| count).sum();
        if !lod_applies(rows_in_span, cols.width_px) {
            return None;
        }
        let resident = &self.resident_crosses;
        let span_rows = runs
            .into_iter()
            .flat_map(|(start, count)| start..start + count)
            .map(|slot| {
                let c = &resident[slot];
                (slot as u32, c.time_rel, c.price, c.side, c.qty)
            });
        if crosses {
            reduce_crosses(span_rows, cols, &mut self.lod_pick);
        } else {
            reduce_volume(span_rows, cols, &mut self.lod_pick);
        }
        let picked = if crosses {
            &self.lod_pick.cross
        } else {
            &self.lod_pick.volume
        };
        let n = lod_instance_count(rows_in_span, cols.width_px, picked.len());
        self.lod_rows.clear();
        self.lod_rows
            .extend(picked.iter().map(|&slot| resident[slot as usize]));
        let n = n as u32;
        if n == 0 {
            return Some(0);
        }
        let pipe = self.pipe.as_mut().unwrap();
        if n > pipe.lod_capacity {
            pipe.lod_capacity = n.next_power_of_two();
            pipe.lod_buffer = create_structured(
                device,
                std::mem::size_of::<ChartCross>() as u32,
                pipe.lod_capacity,
            );
            pipe.lod_srv = create_srv(device, &pipe.lod_buffer);
        }
        update_dynamic(context, &pipe.lod_buffer, &self.lod_rows);
        Some(n)
    }

    /// Draw ring runs into one bitmap with one shader pair, clearing it first on a full bake.
    /// Runs stay in ascending physical slot order so overlaps blend as in a direct draw.
    fn bake_pass(
        context: &ID3D11DeviceContext,
        pipe: &CrossPipe,
        rtv: &ID3D11RenderTargetView,
        bake_view: ChartViewGpu,
        (vs, ps): (&ID3D11VertexShader, &ID3D11PixelShader),
        rows: &ID3D11ShaderResourceView,
        runs: [(u32, u32); 2],
        clear: bool,
    ) {
        let [w, h] = bake_view.resolution;
        let vp = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: w,
            Height: h,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            context.RSSetViewports(Some(&[vp]));
            set_scissor_rect(context, 0.0, 0.0, w, h);
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            let cbs = [
                Some(pipe.view_cb.clone()),
                None,
                Some(pipe.tick_style_cb.clone()),
            ];
            context.VSSetConstantBuffers(0, Some(&cbs));
            context.PSSetConstantBuffers(0, Some(&cbs));
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            if clear {
                // Keep the bitmap background transparent so alpha blitting reveals the grid below.
                context.ClearRenderTargetView(rtv, &[0.0, 0.0, 0.0, 0.0]);
            }
            context.VSSetShaderResources(1, Some(&[Some(rows.clone())]));
            context.VSSetShader(vs, None);
            context.PSSetShader(ps, None);
            for (first, count) in runs {
                if count == 0 {
                    continue;
                }
                let mut run_view = bake_view;
                run_view.pad = first as f32;
                update_dynamic(context, &pipe.view_cb, &[run_view]);
                context.DrawInstanced(6, count, 0, 0);
            }
        }
    }

    /// Composite the volume band, then the crosses over it, each through its whole-texel UV
    /// window; point sampling at a fractional offset would flicker by half a pixel.
    fn blit_combo(
        &mut self,
        view: &ChartViewGpu,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
        panel_clip: [f32; 4],
    ) {
        let [x, y, bw, bh] = view.bounds;
        let Some(pipe) = self.pipe.as_ref() else {
            return;
        };
        if bw <= 0.0 {
            return;
        }
        let mut layers: [Option<(&ID3D11ShaderResourceView, BlitParams)>; 2] = [None, None];
        if let Some(vol) = self.vol_tex.as_ref().filter(|v| v.key.valid) {
            let band = vol.key.band_px as f32;
            let (uv_off, uv_scale) = volume_blit_uv(&vol.key, view);
            layers[0] = Some((
                &vol.srv,
                BlitParams {
                    dst: [x, y + bh - band, bw, band],
                    resolution: view.resolution,
                    uv_off,
                    uv_scale,
                    pad: [0.0, 0.0],
                },
            ));
        }
        if let Some(tex) = self.tex.as_ref().filter(|t| t.key.valid) {
            let (uv_off, uv_scale) = cross_blit_uv(&tex.key, view);
            layers[1] = Some((
                &tex.srv,
                BlitParams {
                    dst: view.bounds,
                    resolution: view.resolution,
                    uv_off,
                    uv_scale,
                    pad: [0.0, 0.0],
                },
            ));
        }
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
            context.VSSetShader(&pipe.blit_vs, None);
            context.PSSetShader(&pipe.blit_fs, None);
            context.VSSetConstantBuffers(0, Some(&[Some(pipe.blit_cb.clone())]));
            context.PSSetConstantBuffers(0, Some(&[Some(pipe.blit_cb.clone())]));
            context.PSSetSamplers(0, Some(&[Some(pipe.sampler.clone())]));
            context.OMSetBlendState(&pipe.premultiplied_blend, None, 0xFFFFFFFF);
            for (srv, bp) in layers.into_iter().flatten() {
                update_dynamic(context, &pipe.blit_cb, &[bp]);
                context.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
                context.Draw(6, 0);
            }
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
        update_dynamic(context, &pipe.price_style_cb, &[self.price_style]);
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
            context.VSSetConstantBuffers(
                0,
                Some(&[
                    Some(pipe.view_cb.clone()),
                    Some(pipe.price_style_cb.clone()),
                ]),
            );
            context.PSSetConstantBuffers(
                0,
                Some(&[
                    Some(pipe.view_cb.clone()),
                    Some(pipe.price_style_cb.clone()),
                ]),
            );
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            Self::draw_price_lines(context, pipe, self.last_line_count, self.mark_line_count);
        }
    }

    /// Upload pending rows; offscreen eviction keeps the established incremental append look.
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
            let full_reset = data.len() >= cap as usize;
            let invalidates_bake = self.tex.as_ref().is_some_and(|tex| {
                self.append_invalidates_bake(
                    data.len(),
                    tick_bake_span(
                        tex.key.bake_t0,
                        tex.key.tex_w as f32,
                        tex.key.time_to_px,
                        tex.key.marker_half,
                    ),
                )
            }) || self.vol_tex.as_ref().is_some_and(|tex| {
                self.append_invalidates_bake(
                    data.len(),
                    tick_bake_span(
                        tex.key.bake_t0,
                        tex.key.tex_w as f32,
                        tex.key.time_to_px,
                        0.0,
                    ),
                )
            });
            let n = data.len() as u32;
            // A capacity-sized batch resets the CPU mirror to slot zero, regardless of old head.
            let written =
                !full_reset && ring_write_no_overwrite(context, &tick_buffer, self.head, cap, data);
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
            if !written {
                // A refused in-place append or a full reset requires a DISCARD upload of the
                // mirror's actual slot layout. Otherwise a bounded draw could select different
                // CPU/GPU ticks. Its head and count become the GPU ring's as well.
                update_dynamic(context, &tick_buffer, &self.resident_crosses);
                self.head = self.resident_head as u32;
                self.count = self.resident_count as u32;
                self.invalidate_bakes();
            }
            // prepare_combo compares the new bake-window scale with the scale actually baked.
            // A global maximum (including an evicted offscreen maximum) cannot affect that scale.
            self.volume_data_generation = self.volume_data_generation.wrapping_add(1);
            self.volume_window_cache = None;
            // New runs draw into each bitmap separately, so crosses always sit above the bars.
            if invalidates_bake {
                self.invalidate_bakes();
            }
        }
    }

    /// Repaint only when an append replaces the ring or erases a potentially baked row.
    /// Overlap with surviving/new ticks keeps the pre-existing incremental append order.
    fn append_invalidates_bake(&self, appended: usize, baked_span: (f64, f64)) -> bool {
        appended >= self.cross_capacity as usize
            || evicted_cross_ranges(
                self.resident_head,
                self.resident_count,
                self.cross_capacity as usize,
                appended,
            )
            .into_iter()
            .any(|(start, count)| {
                self.resident_crosses[start..start + count]
                    .iter()
                    .any(|cross| tick_touches_bake(cross.time_rel, baked_span))
            })
    }

    /// Borrow candidates including pending uploads using O(1) slice probes and lateness bounds.
    pub(super) fn tick_samples(&self, from: f64, to: f64) -> impl Iterator<Item = &ChartCross> {
        let samples = moon_chart::tick_volume::pending_ring(
            &self.resident_crosses,
            self.resident_head,
            self.resident_count,
            self.cross_capacity as usize,
            self.pending_reset.as_deref(),
            &self.pending_append,
        );
        let range = tick_time_range(
            samples.len(),
            self.tick_time_order.max_lateness(),
            from,
            to,
            |index| {
                pending_ring_at(
                    &self.resident_crosses,
                    self.resident_head,
                    self.resident_count,
                    self.cross_capacity as usize,
                    self.pending_reset.as_deref(),
                    &self.pending_append,
                    index,
                )
                .time_rel
            },
        );
        samples.skip(range.start).take(range.len())
    }

    /// Search resident rows without changing their physical GPU slot layout.
    fn resident_time_range(&self, from: f64, to: f64) -> std::ops::Range<usize> {
        let capacity = self.cross_capacity as usize;
        let origin = if self.resident_count == capacity {
            self.resident_head
        } else {
            0
        };
        tick_time_range(
            self.resident_count,
            self.tick_time_order.max_lateness(),
            from,
            to,
            |index| self.resident_crosses[(origin + index) % capacity].time_rel,
        )
    }

    /// Cache the exact historical volume-window predicate over a bounded ring lookup.
    fn volume_scale_for_bake_window(
        &mut self,
        bake_t0: f32,
        tex_w: f32,
        time_to_px: f32,
    ) -> (f32, f32) {
        if !(time_to_px > 1e-9) || self.resident_count == 0 {
            return (1e-6, 1e-6);
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
        let range = self.resident_time_range(f64::from(time_left), f64::from(time_right));
        let runs = tick_slot_runs(
            range,
            self.resident_head,
            self.resident_count,
            self.cross_capacity as usize,
        );
        let mut buy = 1e-6f32;
        let mut sell = 1e-6f32;
        for c in runs
            .into_iter()
            .flat_map(|(start, count)| &self.resident_crosses[start..start + count])
        {
            if c.time_rel < time_left || c.time_rel > time_right || c.qty <= 0.0 {
                continue;
            }
            match c.side {
                0 => buy = buy.max(c.qty),
                1 => sell = sell.max(c.qty),
                _ => {} // Sides >= 2 are liquidations without volume bars, so exclude them from scale.
            }
        }
        let out = (buy, sell);
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

    /// Creates tick, volume, and price pipelines with their style buffers.
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
        let lod_capacity = MIN_COMBO_CAPACITY;
        let lod_buffer = create_structured(
            device,
            std::mem::size_of::<ChartCross>() as u32,
            lod_capacity,
        );
        let lod_srv = create_srv(device, &lod_buffer);
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
        let price_style_cb = create_dynamic_cb(device, std::mem::size_of::<PriceStyleGpu>() as u32);
        let tick_style_cb = create_dynamic_cb(device, std::mem::size_of::<TickStyleGpu>() as u32);
        let blit_vs = super::gpu::make_vs(device, BLIT_HLSL, "blit_vertex");
        let blit_fs = super::gpu::make_ps(device, BLIT_HLSL, "blit_fragment");
        let blit_cb = create_dynamic_cb(device, std::mem::size_of::<BlitParams>() as u32);
        let sampler = create_point_sampler(device);
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
            lod_buffer,
            lod_srv,
            lod_capacity,
            last_line_buf,
            last_line_srv,
            mark_line_buf,
            mark_line_srv,
            view_cb,
            price_style_cb,
            tick_style_cb,
            blit_vs,
            blit_fs,
            blit_cb,
            sampler,
        }
    }

    fn create_tex(device: &ID3D11Device, tex_w: u32, tex_h_total: u32, v_margin: f32) -> ComboTex {
        let (tex, rtv, srv) = super::gpu::create_cache_texture(device, tex_w, tex_h_total);
        ComboTex {
            _tex: tex,
            rtv,
            srv,
            key: ComboBakeKey::unbaked(tex_w, tex_h_total, v_margin),
            last_baked_head: u32::MAX,
        }
    }

    fn create_vol_tex(device: &ID3D11Device, tex_w: u32, band_px: u32, chart_h: u32) -> VolumeTex {
        let (tex, rtv, srv) = super::gpu::create_cache_texture(device, tex_w, band_px);
        VolumeTex {
            _tex: tex,
            rtv,
            srv,
            key: VolumeBakeKey::unbaked(tex_w, band_px, chart_h),
            last_baked_head: u32::MAX,
        }
    }
}

/// Ring runs `[last, head)` appended since a bake, split at the wrap in draw order.
fn appended_runs(last: u32, head: u32, cap: u32) -> [(u32, u32); 2] {
    let delta = (head + cap - last) % cap;
    if last + delta <= cap {
        [(last, delta), (0, 0)]
    } else {
        [(last, cap - last), (0, delta - (cap - last))]
    }
}

fn slot_runs_u32(runs: [(usize, usize); 2]) -> [(u32, u32); 2] {
    runs.map(|(first, count)| (first as u32, count as u32))
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

#[cfg(test)]
mod tests;
