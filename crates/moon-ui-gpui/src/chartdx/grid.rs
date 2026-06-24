//! Слой сетки (хром данных): СТАТИЧНЫЕ вертикали (фикс. X-деления) + горизонтали по цене.
//! Процедурный fullscreen-проход над chart_area (1 drawcall). Рисуется ПЕРВЫМ в нашем
//! own-pass — под крестами/данными. Вертикали не «едут» (модель MoonBot).

use gpui::RawGpuAccess;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::gpu::{
    create_alpha_blend, create_dynamic_cb, full_viewport, make_ps, make_vs, update_dynamic,
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

    /// Рисует сетку в backbuffer хука (под данными). `params.resolution` ставит вызывающий
    /// (= размер backbuffer). bounds — chart_area в координатах окна.
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
