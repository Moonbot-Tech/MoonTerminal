//! Native cursor readout background overlay for the Windows DX11 chart backend.
//!
//! Text itself is emitted by `gpu_canvas.prepare_text` so it uses the same glyph
//! rendering path as the rest of GPUI text. This layer only draws readout chips.

use gpui::RawGpuAccess;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::gpu::{
    create_alpha_blend, create_srv, create_structured, full_viewport, make_ps, make_vs,
    update_dynamic,
};
use super::types::ReadoutRect;

const HLSL: &str = include_str!("shaders/readout.hlsl");
const INITIAL_READOUT_RECT_BUFFER_CAPACITY: u32 = 4;

struct ReadoutPipe {
    rect_vs: ID3D11VertexShader,
    rect_ps: ID3D11PixelShader,
    blend: ID3D11BlendState,
    rect_buf: ID3D11Buffer,
    rect_srv: ID3D11ShaderResourceView,
    rect_cap: u32,
}

pub struct ReadoutLayer {
    pipe: Option<ReadoutPipe>,
    device_generation: u64,
}

impl ReadoutLayer {
    pub fn new() -> Self {
        Self {
            pipe: None,
            device_generation: 0,
        }
    }

    pub fn render(
        &mut self,
        rects: &[ReadoutRect],
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
    ) {
        if rects.is_empty() {
            return;
        }
        let generation = gpu.device_generation();
        if self.device_generation != generation {
            self.pipe = None;
            self.device_generation = generation;
        }
        if self.pipe.is_none() {
            self.pipe = Some(Self::create_pipe(
                device,
                INITIAL_READOUT_RECT_BUFFER_CAPACITY,
            ));
        }
        let rect_cap = next_buffer_cap(rects.len(), INITIAL_READOUT_RECT_BUFFER_CAPACITY);
        if self.pipe.as_ref().is_none_or(|p| p.rect_cap < rect_cap) {
            self.pipe = Some(Self::create_pipe(device, rect_cap));
        }
        let pipe = self.pipe.as_ref().unwrap();
        update_dynamic(context, &pipe.rect_buf, rects);

        let vp = full_viewport(gpu);
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            context.RSSetViewports(Some(&[vp]));
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            context.VSSetShaderResources(1, Some(&[Some(pipe.rect_srv.clone())]));
            context.VSSetShader(&pipe.rect_vs, None);
            context.PSSetShader(&pipe.rect_ps, None);
            context.DrawInstanced(6, rects.len() as u32, 0, 0);
        }
    }

    fn create_pipe(device: &ID3D11Device, rect_cap: u32) -> ReadoutPipe {
        let rect_buf =
            create_structured(device, std::mem::size_of::<ReadoutRect>() as u32, rect_cap);
        ReadoutPipe {
            rect_vs: make_vs(device, HLSL, "readout_rect_vertex"),
            rect_ps: make_ps(device, HLSL, "readout_rect_fragment"),
            blend: create_alpha_blend(device),
            rect_srv: create_srv(device, &rect_buf),
            rect_cap,
            rect_buf,
        }
    }
}

fn next_buffer_cap(len: usize, floor: u32) -> u32 {
    let need = (len as u32).max(1);
    floor.max(need).next_power_of_two()
}
