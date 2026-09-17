//! D3D11 own-pass layer for the horizontal volumes (Moonbot's `HVol`): turnover by price over a
//! trailing window, drawn as rows in a zone of their own beside the plot — or, with the tab's
//! `hvol_overlay` on, over the plot's own left strip. Drawn in the base pass after the order
//! zones and BEFORE the candles: laid over the plot the rows must sit under the bodies and the
//! crosses like the bottom band, and carved out beside it nothing else draws in their zone, so
//! one order serves both. The base is where a carved zone's backdrop belongs, and the overlaid
//! zone has none: the shaders skip that pass on the flag.
//!
//! The base is redrawn only on data changes or camera movement, so this layer adds no work to
//! the presentation path. The buffer holds the zone's per-pixel rolling samples and is reuploaded
//! in full whenever they are retaken: when the profile's revision moved on the source's own slow
//! clock (see `moon_core::market::source::volume::profile`), when the window changed, or when
//! the Y camera moved — which is a base rebake anyway.

use gpui::RawGpuAccess;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::gpu::{
    create_alpha_blend, create_dynamic_cb, create_srv, create_structured, device_changed,
    full_viewport, set_scissor_rect, update_dynamic,
};
use super::types::{ChartViewGpu, HVOL_BG_INSTANCES, HVOL_CAPACITY, HvolRowGpu, HvolStyleGpu};

/// Row capacity of the GPU buffer — the upload is already cut to this in `fill_hvol_upload`, so
/// the truncation below is a guard, not the rule.
const ROW_CAPACITY: u32 = HVOL_CAPACITY as u32;
const HVOL_HLSL: &str = include_str!("shaders/hvol.hlsl");

struct HvolPipe {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    bg_vs: ID3D11VertexShader,
    bg_ps: ID3D11PixelShader,
    style_cb: ID3D11Buffer,
    blend: ID3D11BlendState,
    buffer: ID3D11Buffer,
    srv: ID3D11ShaderResourceView,
    view_cb: ID3D11Buffer,
}

pub struct HvolLayer {
    pipe: Option<HvolPipe>,
    pending: Option<Vec<HvolRowGpu>>,
    count: u32,
    style: HvolStyleGpu,
    style_dirty: bool,
    device_generation_seen: u64,
}

impl HvolLayer {
    pub fn new() -> Self {
        Self {
            pipe: None,
            pending: None,
            count: 0,
            style: HvolStyleGpu::default(),
            style_dirty: true,
            device_generation_seen: 0,
        }
    }

    /// Fully replaces the sample set after it was retaken.
    pub fn set(&mut self, data: Vec<HvolRowGpu>) {
        self.pending = Some(data);
    }

    /// Idempotently sets the zone, colours, kind and normalisation.
    pub fn set_style(&mut self, style: HvolStyleGpu) {
        if self.style != style {
            self.style = style;
            self.style_dirty = true;
        }
    }

    /// Whether this layer draws: a zone exists. Rows may be absent (a market whose history the
    /// core has not sent yet) and the backdrop still draws.
    pub fn has_data(&self) -> bool {
        self.style.zone[2] >= 1.0
    }

    /// Uploads the pending buffer and style constants during `prepare_gpu`.
    pub fn prepare(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        gpu: &RawGpuAccess,
    ) {
        if device_changed(&mut self.device_generation_seen, gpu) {
            self.pipe = None;
            self.count = 0;
            self.style_dirty = true;
        }
        if self.pipe.is_none() {
            self.pipe = Some(Self::create_pipe(device));
        }
        let pipe = self.pipe.as_ref().unwrap();
        if let Some(data) = self.pending.take() {
            let cap = ROW_CAPACITY as usize;
            let data: &[HvolRowGpu] = if data.len() > cap {
                &data[..cap]
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

    /// Draws the backdrop and the rows in the base pass.
    pub fn render(
        &mut self,
        view: &ChartViewGpu,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
        panel_clip: [f32; 4],
    ) {
        if !self.has_data() {
            return;
        }
        let Some(pipe) = self.pipe.as_ref() else {
            return;
        };
        crate::diag::bump(&crate::diag::CHART_HVOL_DRAW);
        update_dynamic(context, &pipe.view_cb, std::slice::from_ref(view));
        let vp = full_viewport(gpu);
        // Clipped to the ZONE, not to the pane's plot-to-book span the other layers use: carved
        // out, the zone sits left of the plot, outside that span; laid over the plot, the clip
        // keeps the rows inside their strip. Its vertical extent stays the pane's.
        let zone = self.style.zone;
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            context.RSSetViewports(Some(&[vp]));
            set_scissor_rect(
                context,
                zone[0],
                panel_clip[1],
                zone[0] + zone[2],
                panel_clip[3],
            );
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            let cbs = [Some(pipe.view_cb.clone()), Some(pipe.style_cb.clone())];
            context.VSSetConstantBuffers(0, Some(&cbs));
            context.PSSetConstantBuffers(0, Some(&cbs));
            context.VSSetShaderResources(3, Some(&[Some(pipe.srv.clone())]));
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            context.VSSetShader(&pipe.bg_vs, None);
            context.PSSetShader(&pipe.bg_ps, None);
            context.DrawInstanced(6, HVOL_BG_INSTANCES, 0, 0);
            // Twelve vertices per row: the bought length, then the sold one over it.
            if self.count > 0 {
                context.VSSetShader(&pipe.vs, None);
                context.PSSetShader(&pipe.ps, None);
                context.DrawInstanced(12, self.count, 0, 0);
            }
        }
    }

    fn create_pipe(device: &ID3D11Device) -> HvolPipe {
        let vs = super::gpu::make_vs(device, HVOL_HLSL, "hvol_row_vertex");
        let ps = super::gpu::make_ps(device, HVOL_HLSL, "hvol_row_fragment");
        let bg_vs = super::gpu::make_vs(device, HVOL_HLSL, "hvol_bg_vertex");
        let bg_ps = super::gpu::make_ps(device, HVOL_HLSL, "hvol_bg_fragment");
        let blend = create_alpha_blend(device);
        let buffer = create_structured(
            device,
            std::mem::size_of::<HvolRowGpu>() as u32,
            ROW_CAPACITY,
        );
        let srv = create_srv(device, &buffer);
        let view_cb = create_dynamic_cb(device, std::mem::size_of::<ChartViewGpu>() as u32);
        let style_cb = create_dynamic_cb(device, std::mem::size_of::<HvolStyleGpu>() as u32);
        HvolPipe {
            vs,
            ps,
            bg_vs,
            bg_ps,
            style_cb,
            blend,
            buffer,
            srv,
            view_cb,
        }
    }
}
