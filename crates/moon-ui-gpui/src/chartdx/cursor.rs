//! Native cursor/crosshair overlay for the Windows DX11 chart backend.

use gpui::RawGpuAccess;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::gpu::{
    create_alpha_blend, create_dynamic_cb, full_viewport, make_ps, make_vs, update_dynamic,
};
pub use super::types::CursorParams;

const CURSOR_HLSL: &str = include_str!("shaders/cursor.hlsl");

struct CursorPipe {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    blend: ID3D11BlendState,
    cb: ID3D11Buffer,
}

pub struct CursorLayer {
    pipe: Option<CursorPipe>,
    device_generation: u64,
}

impl CursorLayer {
    pub fn new() -> Self {
        Self {
            pipe: None,
            device_generation: 0,
        }
    }

    pub fn render(
        &mut self,
        params: &CursorParams,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
    ) {
        if params.bounds[2] <= 0.0 || params.bounds[3] <= 0.0 || params.enabled <= 0.0 {
            return;
        }
        let generation = gpu.device_generation();
        if self.device_generation != generation {
            self.pipe = None;
            self.device_generation = generation;
        }
        if self.pipe.is_none() {
            self.pipe = Some(Self::create_pipe(device));
        }
        let pipe = self.pipe.as_ref().unwrap();
        update_dynamic(context, &pipe.cb, &[*params]);
        let vp = full_viewport(gpu);
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            context.RSSetViewports(Some(&[vp]));
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&pipe.vs, None);
            context.PSSetShader(&pipe.ps, None);
            context.VSSetConstantBuffers(0, Some(&[Some(pipe.cb.clone())]));
            context.PSSetConstantBuffers(0, Some(&[Some(pipe.cb.clone())]));
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            context.Draw(12, 0);
        }
    }

    fn create_pipe(device: &ID3D11Device) -> CursorPipe {
        let vs = make_vs(device, CURSOR_HLSL, "cursor_vertex");
        let ps = make_ps(device, CURSOR_HLSL, "cursor_fragment");
        let blend = create_alpha_blend(device);
        let cb = create_dynamic_cb(device, std::mem::size_of::<CursorParams>() as u32);
        CursorPipe { vs, ps, blend, cb }
    }
}
