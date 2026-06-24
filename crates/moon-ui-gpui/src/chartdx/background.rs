//! Статичная фото-подложка чарта. Рисуется первым own-pass слоем, под сеткой и данными.

use std::ffi::c_void;

use gpui::RawGpuAccess;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC};

use super::gpu::{
    create_alpha_blend, create_dynamic_cb, create_point_sampler, full_viewport, make_ps, make_vs,
    update_dynamic,
};
pub use super::types::BackgroundParams;

/// Пер-панельный водяной знак в плоте (off по умолчанию: CHART_PHOTO_BACKGROUND_ENABLED).
pub const BACKGROUND_3DLOGO_PNG: &[u8] = include_bytes!("../../../../assets/img/3Dlogo_s01.png");
/// Брендовый сплэш — полно-оконная подложка под панелями (убирает белый фон жёлоба/пустот).
pub const SPLASH_PNG: &[u8] = include_bytes!("../../../../assets/img/splash-cold-glow.png");
const BACKGROUND_HLSL: &str = include_str!("shaders/background.hlsl");

struct BackgroundPipe {
    _tex: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    sampler: ID3D11SamplerState,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    blend: ID3D11BlendState,
    cb: ID3D11Buffer,
}

pub struct BackgroundLayer {
    pipe: Option<BackgroundPipe>,
    device_generation: u64,
    png: &'static [u8],
}

impl BackgroundLayer {
    pub fn new(png: &'static [u8]) -> Self {
        Self {
            pipe: None,
            device_generation: 0,
            png,
        }
    }

    pub fn render(
        &mut self,
        params: &BackgroundParams,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
    ) {
        if params.dst[2] <= 0.0 || params.dst[3] <= 0.0 {
            return;
        }
        let generation = gpu.device_generation();
        if self.device_generation != generation {
            self.pipe = None;
            self.device_generation = generation;
        }
        if self.pipe.is_none() {
            self.pipe = Some(Self::create_pipe(device, self.png));
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
            context.PSSetShaderResources(0, Some(&[Some(pipe.srv.clone())]));
            context.PSSetSamplers(0, Some(&[Some(pipe.sampler.clone())]));
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            context.Draw(6, 0);
        }
    }

    fn create_pipe(device: &ID3D11Device, png: &[u8]) -> BackgroundPipe {
        let image = image::load_from_memory(png)
            .expect("embedded chart background must decode")
            .to_rgba8();
        let img_w = image.width();
        let img_h = image.height();
        let desc = D3D11_TEXTURE2D_DESC {
            Width: img_w,
            Height: img_h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_IMMUTABLE,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let initial = D3D11_SUBRESOURCE_DATA {
            pSysMem: image.as_ptr() as *const c_void,
            SysMemPitch: img_w * 4,
            SysMemSlicePitch: 0,
        };
        let tex = unsafe {
            let mut out = None;
            device
                .CreateTexture2D(&desc, Some(&initial), Some(&mut out))
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
        BackgroundPipe {
            _tex: tex,
            srv,
            sampler: create_point_sampler(device),
            vs: make_vs(device, BACKGROUND_HLSL, "background_vertex"),
            ps: make_ps(device, BACKGROUND_HLSL, "background_fragment"),
            blend: create_alpha_blend(device),
            cb: create_dynamic_cb(device, std::mem::size_of::<BackgroundParams>() as u32),
        }
    }
}
