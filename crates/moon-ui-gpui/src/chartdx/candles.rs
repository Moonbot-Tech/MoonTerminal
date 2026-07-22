//! D3D11 own-pass candle layer, drawn with instancing in the base pass between the grid and
//! combo layers so candles remain below trade crosses. The base is redrawn only on data changes
//! or camera movement at the existing cadence, so this layer adds no work to the presentation
//! path. The buffer, containing hundreds of candles, is cheaply reuploaded in full whenever the
//! series revision changes. `CandleStyle` constants control the mode, zone, outline, and colors
//! without rebuilding vertices.

use gpui::RawGpuAccess;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::gpu::{
    create_alpha_blend, create_dynamic_cb, create_srv, create_structured, device_changed,
    full_viewport, set_scissor_rect, update_dynamic,
};
use super::types::{CandleGpu, CandleStyleGpu, ChartViewGpu};

/// Candle instance capacity of the GPU buffer. The visible window plus prefetch uses hundreds;
/// 4096 leaves ample headroom while consuming 96 KB of VRAM at 24 bytes per instance. On
/// overflow, only the most recent tail is retained.
const CANDLE_CAPACITY: u32 = 4096;
const CANDLES_HLSL: &str = include_str!("shaders/candles.hlsl");

struct CandlePipe {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
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
        }
        if self.pipe.is_none() {
            self.pipe = Some(Self::create_pipe(device));
        }
        let pipe = self.pipe.as_ref().unwrap();
        if let Some(data) = self.pending.take() {
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
        }
        if self.style_dirty {
            update_dynamic(context, &pipe.style_cb, &[self.style]);
            self.style_dirty = false;
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
            context.VSSetConstantBuffers(
                0,
                Some(&[Some(pipe.view_cb.clone()), Some(pipe.style_cb.clone())]),
            );
            context.PSSetConstantBuffers(
                0,
                Some(&[Some(pipe.view_cb.clone()), Some(pipe.style_cb.clone())]),
            );
            context.VSSetShaderResources(3, Some(&[Some(pipe.srv.clone())]));
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            context.VSSetShader(&pipe.vs, None);
            context.PSSetShader(&pipe.ps, None);
            // Use 18 vertices per candle: the body and upper/lower wicks form three quads.
            context.DrawInstanced(18, self.count, 0, 0);
        }
    }

    fn create_pipe(device: &ID3D11Device) -> CandlePipe {
        let vs = super::gpu::make_vs(device, CANDLES_HLSL, "candles_vertex");
        let ps = super::gpu::make_ps(device, CANDLES_HLSL, "candles_fragment");
        let blend = create_alpha_blend(device);
        let buffer = create_structured(
            device,
            std::mem::size_of::<CandleGpu>() as u32,
            CANDLE_CAPACITY,
        );
        let srv = create_srv(device, &buffer);
        let view_cb = create_dynamic_cb(device, std::mem::size_of::<ChartViewGpu>() as u32);
        let style_cb = create_dynamic_cb(device, std::mem::size_of::<CandleStyleGpu>() as u32);
        CandlePipe {
            vs,
            ps,
            blend,
            buffer,
            srv,
            view_cb,
            style_cb,
        }
    }
}
