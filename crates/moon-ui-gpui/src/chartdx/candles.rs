//! D3D11 own-pass candle layer, drawn with instancing in the base pass between the grid and
//! combo layers so candles remain below trade crosses. The base is redrawn only on data changes
//! or camera movement at the existing cadence, so this layer adds no work to the presentation
//! path. The buffer is reuploaded IN FULL whenever the series revision changes — which a live
//! trade batch does, so on a live market that is continuous: `candle_upload_len` measures 9 000 to
//! 83 000 rows a second across a handful of charts, not the "hundreds" this doc used to claim.
//! `candle_upload_us` is what it costs, both halves of it — building the vector here and mapping
//! it in `prepare` — and the answer is why the full reupload survives: under a millisecond a
//! second on average. `CandleStyle` constants control the mode, zone, outline, and colors without
//! rebuilding vertices.

use gpui::RawGpuAccess;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::gpu::{
    create_alpha_blend, create_dynamic_cb, create_srv, create_structured, device_changed,
    full_viewport, set_scissor_rect, update_dynamic,
};
use super::types::{CandleGpu, CandleStyleGpu, ChartViewGpu, VolumeStyleGpu};

/// Candle instance capacity of the GPU buffer: 96 KB of VRAM at 24 bytes per instance. On
/// overflow only the most recent tail is retained, which silently drops the left of the chart.
///
/// "The visible window plus prefetch uses hundreds" is what this said, and it is not what the
/// series measures: around a thousand rows per upload on a zoomed-out chart, and the series grows
/// with the visible range. The headroom is real — roughly four times the observed size — but it is
/// headroom, not the order of magnitude the number was picked against. Zooming out far enough is
/// the thing that would reach it, and it would show as candles missing on the left rather than as
/// an error.
const CANDLE_CAPACITY: u32 = 4096;
const CANDLES_HLSL: &str = include_str!("shaders/candles.hlsl");

struct CandlePipe {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    volume_vs: ID3D11VertexShader,
    volume_ps: ID3D11PixelShader,
    scale_vs: ID3D11VertexShader,
    scale_ps: ID3D11PixelShader,
    volume_cb: ID3D11Buffer,
    blend: ID3D11BlendState,
    buffer: ID3D11Buffer,
    srv: ID3D11ShaderResourceView,
    view_cb: ID3D11Buffer,
    style_cb: ID3D11Buffer,
}

pub struct CandleLayer {
    pipe: Option<CandlePipe>,
    pending: Option<Vec<CandleGpu>>,
    count: u32,
    style: CandleStyleGpu,
    style_dirty: bool,
    volume_style: VolumeStyleGpu,
    volume_dirty: bool,
    device_generation_seen: u64,
}

impl CandleLayer {
    pub fn new() -> Self {
        Self {
            pipe: None,
            pending: None,
            count: 0,
            style: CandleStyleGpu::default(),
            style_dirty: true,
            volume_style: VolumeStyleGpu::default(),
            volume_dirty: true,
            device_generation_seen: 0,
        }
    }

    /// Fully replaces the candle set after a series revision change.
    pub fn set(&mut self, data: Vec<CandleGpu>) {
        self.pending = Some(data);
    }

    /// Idempotently sets the layer's mode, zone, colors, and outline style.
    pub fn set_style(&mut self, style: CandleStyleGpu) {
        if self.style != style {
            self.style = style;
            self.style_dirty = true;
        }
    }

    /// Idempotently sets the bottom-volume band style.
    pub fn set_volume_style(&mut self, style: VolumeStyleGpu) {
        if self.volume_style != style {
            self.volume_style = style;
            self.volume_dirty = true;
        }
    }

    /// Uploads the pending buffer and style constants during `prepare_gpu`.
    pub fn prepare(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        gpu: &RawGpuAccess,
    ) {
        if device_changed(&mut self.device_generation_seen, gpu) {
            // A lost device invalidates these resources. The data-state orchestrator retains the
            // data and reuploads it by revision, so recreating the pipeline and resetting is enough.
            self.pipe = None;
            self.count = 0;
            self.style_dirty = true;
            self.volume_dirty = true;
        }
        if self.pipe.is_none() {
            self.pipe = Some(Self::create_pipe(device));
        }
        let pipe = self.pipe.as_ref().unwrap();
        if let Some(data) = self.pending.take() {
            // The SECOND half of what `candle_upload_us` measures. `set` only parks the vector;
            // the map-and-copy to the GPU happens here, a frame phase later, so timing only the
            // caller would have named this counter after work it never observed.
            let write_timer = crate::diag::timer();
            let cap = CANDLE_CAPACITY as usize;
            let data: &[CandleGpu] = if data.len() > cap {
                &data[data.len() - cap..]
            } else {
                &data
            };
            if !data.is_empty() {
                update_dynamic(context, &pipe.buffer, data);
            }
            self.count = data.len() as u32;
            crate::diag::record_us(&crate::diag::CHART_CANDLE_UPLOAD_US, write_timer);
        }
        if self.style_dirty {
            update_dynamic(context, &pipe.style_cb, &[self.style]);
            self.style_dirty = false;
        }
        if self.volume_dirty {
            update_dynamic(context, &pipe.volume_cb, &[self.volume_style]);
            self.volume_dirty = false;
        }
    }

    /// Draws candles in the base pass between grid and combo after `prepare()` uploads the data.
    pub fn render(
        &mut self,
        view: &ChartViewGpu,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
        panel_clip: [f32; 4],
    ) {
        if self.count == 0 {
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
            let cbs = [
                Some(pipe.view_cb.clone()),
                Some(pipe.style_cb.clone()),
                Some(pipe.volume_cb.clone()),
            ];
            context.VSSetConstantBuffers(0, Some(&cbs));
            context.PSSetConstantBuffers(0, Some(&cbs));
            context.VSSetShaderResources(3, Some(&[Some(pipe.srv.clone())]));
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            // The volume band and its scale go FIRST so the candle bodies sit on top of them.
            // `m[0]` is the style: 0 is off.
            if self.volume_style.m[0] >= 0.5 {
                crate::diag::bump(&crate::diag::CHART_CANDLE_VOLUME_DRAW);
                // Hills read `candles[iid + 1]`, so they get one instance fewer; bars would
                // otherwise lose the newest bucket, hence the per-style count.
                let bars = if self.volume_style.m[0] >= 1.5 {
                    self.count.saturating_sub(1)
                } else {
                    self.count
                };
                if bars > 0 {
                    context.VSSetShader(&pipe.volume_vs, None);
                    context.PSSetShader(&pipe.volume_ps, None);
                    context.DrawInstanced(6, bars, 0, 0);
                }
                context.VSSetShader(&pipe.scale_vs, None);
                context.PSSetShader(&pipe.scale_ps, None);
                context.DrawInstanced(6, 2, 0, 0);
            }
            context.VSSetShader(&pipe.vs, None);
            context.PSSetShader(&pipe.ps, None);
            // Use 18 vertices per candle: the body and upper/lower wicks form three quads.
            context.DrawInstanced(18, self.count, 0, 0);
        }
    }

    fn create_pipe(device: &ID3D11Device) -> CandlePipe {
        let vs = super::gpu::make_vs(device, CANDLES_HLSL, "candles_vertex");
        let ps = super::gpu::make_ps(device, CANDLES_HLSL, "candles_fragment");
        let volume_vs = super::gpu::make_vs(device, CANDLES_HLSL, "volume_bars_vertex");
        let volume_ps = super::gpu::make_ps(device, CANDLES_HLSL, "volume_bars_fragment");
        let scale_vs = super::gpu::make_vs(device, CANDLES_HLSL, "volume_scale_vertex");
        let scale_ps = super::gpu::make_ps(device, CANDLES_HLSL, "volume_scale_fragment");
        let blend = create_alpha_blend(device);
        let buffer = create_structured(
            device,
            std::mem::size_of::<CandleGpu>() as u32,
            CANDLE_CAPACITY,
        );
        let srv = create_srv(device, &buffer);
        let view_cb = create_dynamic_cb(device, std::mem::size_of::<ChartViewGpu>() as u32);
        let style_cb = create_dynamic_cb(device, std::mem::size_of::<CandleStyleGpu>() as u32);
        let volume_cb = create_dynamic_cb(device, std::mem::size_of::<VolumeStyleGpu>() as u32);
        CandlePipe {
            vs,
            ps,
            volume_vs,
            volume_ps,
            scale_vs,
            scale_ps,
            volume_cb,
            blend,
            buffer,
            srv,
            view_cb,
            style_cb,
        }
    }
}
