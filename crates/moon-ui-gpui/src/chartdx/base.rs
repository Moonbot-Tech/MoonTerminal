//! Full-window cached chart base for DX11.
//!
//! The base texture contains every chart pixel that does not belong to the high-frequency
//! cursor/readout overlay. Cursor-only frames blit this texture and then draw the cursor.

use gpui::RawGpuAccess;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

use super::gpu::{
    BlitParams, create_dynamic_cb, create_point_sampler, create_scissor_rasterizer, full_viewport,
    make_ps, make_vs, set_scissor_rect, update_dynamic,
};

const BLIT_HLSL: &str = include_str!("shaders/blit.hlsl");

struct BaseTex {
    _tex: ID3D11Texture2D,
    rtv: ID3D11RenderTargetView,
    srv: ID3D11ShaderResourceView,
    w: u32,
    h: u32,
    generation: u64,
    blit_vs: ID3D11VertexShader,
    blit_ps: ID3D11PixelShader,
    blit_cb: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    scissor_rs: ID3D11RasterizerState,
}

pub struct BaseCache {
    tex: Option<BaseTex>,
    valid: bool,
}

impl BaseCache {
    pub fn new() -> Self {
        Self {
            tex: None,
            valid: false,
        }
    }

    pub fn is_valid_for(&self, gpu: &RawGpuAccess) -> bool {
        let w = gpu.width();
        let h = gpu.height();
        let generation = gpu.device_generation();
        self.valid
            && self
                .tex
                .as_ref()
                .is_some_and(|tex| tex.w == w && tex.h == h && tex.generation == generation)
    }

    pub fn needs_rebuild(&self, gpu: &RawGpuAccess) -> bool {
        !self.is_valid_for(gpu)
    }

    pub fn begin_rebuild(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        gpu: &RawGpuAccess,
    ) -> anyhow::Result<ID3D11RenderTargetView> {
        let w = gpu.width();
        let h = gpu.height();
        if w == 0 || h == 0 {
            anyhow::bail!("chart base cache cannot rebuild zero-sized target");
        }
        let generation = gpu.device_generation();
        let recreate = self.tex.as_ref().map_or(true, |tex| {
            tex.w != w || tex.h != h || tex.generation != generation
        });
        if recreate {
            self.tex = Some(Self::create_tex(device, w, h, generation));
        }
        let tex = self.tex.as_ref().unwrap();
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(tex.rtv.clone())]), None);
            context.RSSetViewports(Some(&[full_viewport(gpu)]));
            context.ClearRenderTargetView(&tex.rtv, &[0.0, 0.0, 0.0, 0.0]);
        }
        self.valid = true;
        crate::diag::bump(&crate::diag::CHART_BASE_BAKE);
        Ok(tex.rtv.clone())
    }

    /// `clip` (l,t,r,b в device-px backbuffer) — слот ЭТОГО чарта. Блитим строго в него:
    /// при нескольких `gpu_canvas` в одном окне (стек выносного окна) полноэкранный блит
    /// window_bg затирал бы соседние чарты. Текстура база — на весь экран, scissor вырезает слот.
    pub fn blit_to(
        &mut self,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
        clip: [f32; 4],
    ) {
        let Some(tex) = self.tex.as_ref() else {
            return;
        };
        let bp = BlitParams {
            dst: [0.0, 0.0, gpu.width() as f32, gpu.height() as f32],
            resolution: [gpu.width() as f32, gpu.height() as f32],
            uv_off: [0.0, 0.0],
            uv_scale: [1.0, 1.0],
            pad: [0.0, 0.0],
        };
        update_dynamic(context, &tex.blit_cb, &[bp]);
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            context.RSSetViewports(Some(&[full_viewport(gpu)]));
            // Scissor на слот этого чарта: текстура база полноэкранная, но писать в backbuffer
            // можно только в свой слот, иначе затрём соседние gpu_canvas того же окна.
            set_scissor_rect(context, clip[0], clip[1], clip[2], clip[3]);
            context.RSSetState(&tex.scissor_rs);
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&tex.blit_vs, None);
            context.PSSetShader(&tex.blit_ps, None);
            context.VSSetConstantBuffers(0, Some(&[Some(tex.blit_cb.clone())]));
            context.PSSetConstantBuffers(0, Some(&[Some(tex.blit_cb.clone())]));
            context.PSSetShaderResources(0, Some(&[Some(tex.srv.clone())]));
            context.PSSetSamplers(0, Some(&[Some(tex.sampler.clone())]));
            // Blend OFF: непрозрачная замена. База перекрывает белый clear целиком —
            // никакого подмешивания белого через alpha<1 (см. blit_opaque_fragment).
            context.OMSetBlendState(None, None, 0xFFFFFFFF);
            context.Draw(6, 0);
        }
        crate::diag::bump(&crate::diag::CHART_BASE_BLIT);
    }

    fn create_tex(device: &ID3D11Device, w: u32, h: u32, generation: u64) -> BaseTex {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let tex = unsafe {
            let mut out = None;
            device.CreateTexture2D(&desc, None, Some(&mut out)).unwrap();
            out.unwrap()
        };
        let rtv = unsafe {
            let mut out = None;
            device
                .CreateRenderTargetView(&tex, None, Some(&mut out))
                .unwrap();
            out.unwrap()
        };
        let srv = unsafe {
            let mut out = None;
            device
                .CreateShaderResourceView(&tex, None, Some(&mut out))
                .unwrap();
            out.unwrap()
        };
        BaseTex {
            _tex: tex,
            rtv,
            srv,
            w,
            h,
            generation,
            blit_vs: make_vs(device, BLIT_HLSL, "blit_vertex"),
            blit_ps: make_ps(device, BLIT_HLSL, "blit_opaque_fragment"),
            blit_cb: create_dynamic_cb(device, std::mem::size_of::<BlitParams>() as u32),
            sampler: create_point_sampler(device),
            scissor_rs: create_scissor_rasterizer(device),
        }
    }
}
