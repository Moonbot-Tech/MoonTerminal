//! D3D11 own-pass candle layer, drawn with instancing in the base pass between the grid and
//! combo layers so candles remain below trade crosses. The base is redrawn only on data changes
//! or camera movement at the existing cadence, so this layer adds no work to the presentation
//! path. A live trade batch that changes only the tail rewrites only those slots (`CandleWindow`
//! tracks them), and each draw submits only the instances in view. `CandleStyle` constants control
//! the mode, zone, outline, and colors without rebuilding vertices.

mod window;

use window::{CandleUpload, CandleWindow};

use gpui::RawGpuAccess;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::gpu::{
    create_alpha_blend, create_dynamic_cb, create_srv, create_structured_default, device_changed,
    full_viewport, set_scissor_rect, update_default_range, update_dynamic,
};
use super::types::{CandleGpu, CandleStyleGpu, ChartViewGpu, VolumeStyleGpu};

/// Candle instance capacity of the GPU buffer: 112 KB of VRAM at 28 bytes per instance. Taken from
/// `figure_snap::DX11_CANDLE_WINDOW`, which figure snaps mirror.
///
/// Around a thousand instances per upload on a zoomed-out chart, and the series grows with the
/// visible range — so this is roughly four times the observed size, not the order of magnitude it
/// reads as. On overflow only the newest tail is kept and the chart's left simply loses its
/// candles while the grid and trades still draw there; `candle_dropped` counts them.
const CANDLE_CAPACITY: u32 = super::figure_snap::DX11_CANDLE_WINDOW as u32;
const CANDLES_HLSL: &str = include_str!("shaders/candles.hlsl");

struct CandlePipe {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    volume_vs: ID3D11VertexShader,
    volume_ps: ID3D11PixelShader,
    volume_cb: ID3D11Buffer,
    blend: ID3D11BlendState,
    buffer: ID3D11Buffer,
    srv: ID3D11ShaderResourceView,
    view_cb: ID3D11Buffer,
    style_cb: ID3D11Buffer,
}

pub struct CandleLayer {
    pipe: Option<CandlePipe>,
    /// CPU mirror of the buffer and the slots a patched tail dirtied.
    window: CandleWindow,
    /// GPU-resident candle count, the window's length after the last upload.
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
            window: CandleWindow::new(CANDLE_CAPACITY as usize),
            count: 0,
            style: CandleStyleGpu::default(),
            style_dirty: true,
            volume_style: VolumeStyleGpu::default(),
            volume_dirty: true,
            device_generation_seen: 0,
        }
    }

    /// Fully replaces the candle set from the whole composed list.
    pub fn set(&mut self, data: &[CandleGpu]) {
        self.window.set(data);
    }

    /// Re-applies the whole composed list's tail from `from` on; only those slots are uploaded.
    pub fn patch(&mut self, from: usize, full: &[CandleGpu]) {
        self.window.patch(from, full);
    }

    /// Idempotently sets the layer's mode, zone, colors, and outline style.
    pub fn set_style(&mut self, style: CandleStyleGpu) {
        self.window.set_style_tf(style.tf_rel_ms);
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
            self.window.invalidate_gpu();
        }
        if self.pipe.is_none() {
            // A new buffer holds nothing, whatever the window thinks it uploaded before.
            self.pipe = Some(Self::create_pipe(device));
            self.window.invalidate_gpu();
        }
        let pipe = self.pipe.as_ref().unwrap();
        let (upload, rows, dropped) = self.window.take_upload();
        if upload != CandleUpload::None {
            // Second half of `candle_upload_us`: `set`/`patch` only update the CPU mirror, the
            // GPU write happens here, a frame phase later.
            let write_timer = crate::diag::timer();
            match upload {
                CandleUpload::Full => {
                    crate::diag::bump_by(&crate::diag::CHART_CANDLE_DROPPED, dropped);
                    update_default_range(context, &pipe.buffer, 0, rows);
                }
                CandleUpload::Range { first, len } => {
                    update_default_range(
                        context,
                        &pipe.buffer,
                        first as u32,
                        &rows[first..first + len],
                    );
                }
                CandleUpload::None => {}
            }
            self.count = rows.len() as u32;
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
        // Only the instances in view are submitted, with a two-pixel margin either side.
        let (left, right) = if view.time_to_px > 0.0 {
            (
                view.view_time0 - 2.0 / view.time_to_px,
                view.view_time0 + (view.bounds[2] + 2.0) / view.time_to_px,
            )
        } else {
            (f32::NAN, f32::NAN)
        };
        let slice = self.window.draw_slice(left, right).clamped(self.count);
        if slice.candles == 0 {
            return;
        }
        // DX11 candles read the instance offset from `ChartViewGpu.pad` (`cv_instance_offset`),
        // because SV_InstanceID ignores StartInstanceLocation for structured-buffer reads. The base
        // view carries the right-edge time there for other layers and backends, so it is
        // overridden on a local copy only.
        let mut v = *view;
        v.pad = slice.start as f32;
        update_dynamic(context, &pipe.view_cb, std::slice::from_ref(&v));
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
            // The volume band goes FIRST so the candle bodies sit on top of it. `m[0]` is the
            // style: 0 is off. The shader culls the candles past the split boundary, and the
            // sides layer draws the band's scale bracket for both halves.
            if self.volume_style.m[0] >= 0.5 {
                crate::diag::bump(&crate::diag::CHART_CANDLE_VOLUME_DRAW);
                // Hills read `candles[iid + 1]`, so the slice gives them one instance fewer.
                let bars = slice.hills;
                if bars > 0 {
                    context.VSSetShader(&pipe.volume_vs, None);
                    context.PSSetShader(&pipe.volume_ps, None);
                    context.DrawInstanced(6, bars, 0, 0);
                }
            }
            context.VSSetShader(&pipe.vs, None);
            context.PSSetShader(&pipe.ps, None);
            // Use 18 vertices per candle: the body and upper/lower wicks form three quads.
            crate::diag::bump_by(
                &crate::diag::CHART_CANDLE_DRAW_INSTANCES,
                slice.candles as u64,
            );
            context.DrawInstanced(18, slice.candles, 0, 0);
        }
    }

    fn create_pipe(device: &ID3D11Device) -> CandlePipe {
        let vs = super::gpu::make_vs(device, CANDLES_HLSL, "candles_vertex");
        let ps = super::gpu::make_ps(device, CANDLES_HLSL, "candles_fragment");
        let volume_vs = super::gpu::make_vs(device, CANDLES_HLSL, "volume_bars_vertex");
        let volume_ps = super::gpu::make_ps(device, CANDLES_HLSL, "volume_bars_fragment");
        let blend = create_alpha_blend(device);
        let buffer = create_structured_default(
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
            volume_cb,
            blend,
            buffer,
            srv,
            view_cb,
            style_cb,
        }
    }
}
