//! Data-chrome grid layer with static vertical lines at fixed X divisions and horizontal price
//! lines. A procedural full-screen pass over `chart_area` uses one draw call and runs first in the
//! own pass beneath crosses and other data. The vertical lines remain fixed, matching Moonbot.

use gpui::RawGpuAccess;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::gpu::{
    create_alpha_blend, create_dynamic_cb, device_changed, full_viewport, make_ps, make_vs,
    update_dynamic,
};
pub use super::types::GridParams;

const GRID_HLSL: &str = include_str!("shaders/grid.hlsl");

struct GridPipe {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    blend: ID3D11BlendState,
    cb: ID3D11Buffer,
}

pub struct GridLayer {
    pipe: Option<GridPipe>,
    device_generation: u64,
}

impl GridLayer {
    pub fn new() -> Self {
        Self {
            pipe: None,
            device_generation: 0,
        }
    }

    /// Draws the grid beneath data into the hook's backbuffer. The caller sets
    /// `params.resolution` to the backbuffer size; `bounds` is `chart_area` in window coordinates.
    pub fn render(
        &mut self,
        params: &GridParams,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
    ) {
        if params.bounds[2] <= 0.0 || params.bounds[3] <= 0.0 {
            return;
        }
        // device-lost guard: all DX chart layers use RawGpuAccess generation.
        if device_changed(&mut self.device_generation, gpu) {
            self.pipe = None;
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
            context.Draw(6, 0);
        }
    }

    fn create_pipe(device: &ID3D11Device) -> GridPipe {
        let vs = make_vs(device, GRID_HLSL, "grid_vertex");
        let ps = make_ps(device, GRID_HLSL, "grid_fragment");
        let blend = create_alpha_blend(device);
        let cb = create_dynamic_cb(device, std::mem::size_of::<GridParams>() as u32);
        GridPipe { vs, ps, blend, cb }
    }
}
