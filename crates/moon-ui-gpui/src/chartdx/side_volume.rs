//! D3D11 own-pass layer for the bought/sold half of the bottom band (`candle_volume_sides`):
//! rolling sums per sample, drawn in the base pass right AFTER the candle layer so it covers the
//! candle band's last, boundary-straddling bucket; the candle bodies stay under its translucent
//! fill, which is what the reference terminal does too. Its own layer rather than a second draw
//! on the candle layer because it has its own instance buffer with its own cadence — and because
//! it must draw with candles switched OFF, which the candle layer's `count == 0` early return
//! forbids. It also draws the band's scale lines whenever the switch is on: with the candle half
//! read on the same linear scale, one pair of lines serves both.
//!
//! The base is redrawn only on data changes or camera movement, so this layer adds no work to the
//! presentation path. The buffer is reuploaded in full whenever the bucket series is re-read.

use gpui::RawGpuAccess;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::gpu::{
    create_alpha_blend, create_dynamic_cb, create_srv, create_structured, device_changed,
    full_viewport, set_scissor_rect, update_dynamic,
};
use super::types::{ChartViewGpu, SIDE_VOLUME_CAPACITY, SideVolumeGpu, VolumeStyleGpu};

/// Bucket capacity of the GPU buffer — the upload is already cut to this in
/// `fill_side_volume_upload`, so the truncation below is a guard, not the rule.
const SIDE_CAPACITY: u32 = SIDE_VOLUME_CAPACITY as u32;
const SIDE_VOLUME_HLSL: &str = include_str!("shaders/side_volume.hlsl");

struct SidePipe {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    scale_vs: ID3D11VertexShader,
    scale_ps: ID3D11PixelShader,
    style_cb: ID3D11Buffer,
    blend: ID3D11BlendState,
    buffer: ID3D11Buffer,
    srv: ID3D11ShaderResourceView,
    view_cb: ID3D11Buffer,
}

pub struct SideVolumeLayer {
    pipe: Option<SidePipe>,
    pending: Option<Vec<SideVolumeGpu>>,
    count: u32,
    style: VolumeStyleGpu,
    style_dirty: bool,
    device_generation_seen: u64,
}

impl SideVolumeLayer {
    pub fn new() -> Self {
        Self {
            pipe: None,
            pending: None,
            count: 0,
            style: VolumeStyleGpu::default(),
            style_dirty: true,
            device_generation_seen: 0,
        }
    }

    /// Fully replaces the bucket set after the series was re-read.
    pub fn set(&mut self, data: Vec<SideVolumeGpu>) {
        self.pending = Some(data);
    }

    /// Idempotently sets the band style: kind, height, opacity and normalisation.
    ///
    /// The SAME struct the candle layer receives — the two layers read one `VolumeStyle` and the
    /// style id decides which of them draws.
    pub fn set_style(&mut self, style: VolumeStyleGpu) {
        if self.style != style {
            self.style = style;
            self.style_dirty = true;
        }
    }

    /// Whether this layer draws: the sides switch is on. Samples may be absent (a market whose
    /// history the core has not sent yet) and the scale lines still draw for the candle half.
    pub fn has_data(&self) -> bool {
        self.style.m3[0] >= 0.5
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
            let cap = SIDE_CAPACITY as usize;
            let data: &[SideVolumeGpu] = if data.len() > cap {
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

    /// Draws the band and its two reference lines in the base pass, before the candle layer.
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
        crate::diag::bump(&crate::diag::CHART_SIDE_VOLUME_DRAW);
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
            // b1 is the candle layer's CandleStyle slot; this shader declares b0 and b2 only, and
            // the slot between them is left unbound on purpose so the two layouts stay identical.
            let cbs = [
                Some(pipe.view_cb.clone()),
                None,
                Some(pipe.style_cb.clone()),
            ];
            context.VSSetConstantBuffers(0, Some(&cbs));
            context.PSSetConstantBuffers(0, Some(&cbs));
            context.VSSetShaderResources(3, Some(&[Some(pipe.srv.clone())]));
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            // Twelve vertices per bucket: the buy column, then the sell column over it.
            if self.count > 0 {
                context.VSSetShader(&pipe.vs, None);
                context.PSSetShader(&pipe.ps, None);
                context.DrawInstanced(12, self.count, 0, 0);
            }
            context.VSSetShader(&pipe.scale_vs, None);
            context.PSSetShader(&pipe.scale_ps, None);
            context.DrawInstanced(6, 2, 0, 0);
        }
    }

    fn create_pipe(device: &ID3D11Device) -> SidePipe {
        let vs = super::gpu::make_vs(device, SIDE_VOLUME_HLSL, "side_band_vertex");
        let ps = super::gpu::make_ps(device, SIDE_VOLUME_HLSL, "side_band_fragment");
        let scale_vs = super::gpu::make_vs(device, SIDE_VOLUME_HLSL, "side_scale_vertex");
        let scale_ps = super::gpu::make_ps(device, SIDE_VOLUME_HLSL, "side_scale_fragment");
        let blend = create_alpha_blend(device);
        let buffer = create_structured(
            device,
            std::mem::size_of::<SideVolumeGpu>() as u32,
            SIDE_CAPACITY,
        );
        let srv = create_srv(device, &buffer);
        let view_cb = create_dynamic_cb(device, std::mem::size_of::<ChartViewGpu>() as u32);
        let style_cb = create_dynamic_cb(device, std::mem::size_of::<VolumeStyleGpu>() as u32);
        SidePipe {
            vs,
            ps,
            scale_vs,
            scale_ps,
            style_cb,
            blend,
            buffer,
            srv,
            view_cb,
        }
    }
}
