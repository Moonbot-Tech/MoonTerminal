//! Software D3D target for regressions that must verify pixels after native cache operations.

use gpui::{D3d11RawAccess, RawGpuAccess};
use std::{marker::PhantomData, ptr::NonNull};
use windows::{
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_WARP, Direct3D11::*, Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
        },
    },
    core::Interface,
};

/// Own all handles borrowed by a raw callback, including a readable offscreen target.
pub(crate) struct TestTarget {
    pub(crate) device: ID3D11Device,
    pub(crate) context: ID3D11DeviceContext,
    texture: ID3D11Texture2D,
    rtv: ID3D11RenderTargetView,
    width: u32,
    height: u32,
}

impl TestTarget {
    /// WARP is available without a physical GPU; failing to create it must fail the test.
    pub(crate) fn new(width: u32, height: u32) -> Self {
        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .unwrap();
        }
        let device = device.unwrap();
        let context = context.unwrap();
        let (texture, rtv, _) = super::create_cache_texture(&device, width, height);
        let rasterizer = super::create_scissor_rasterizer(&device);
        unsafe {
            context.RSSetState(&rasterizer);
        }
        Self {
            device,
            context,
            texture,
            rtv,
            width,
            height,
        }
    }

    /// Match GPUI's callback ownership and generation contract without a swapchain.
    pub(crate) fn raw(&self, generation: u64) -> RawGpuAccess<'_> {
        RawGpuAccess::D3d11(D3d11RawAccess {
            device_generation: generation,
            device: NonNull::new(self.device.as_raw()).unwrap(),
            context: NonNull::new(self.context.as_raw()).unwrap(),
            render_target: NonNull::new(self.rtv.as_raw()).unwrap(),
            render_target_format: DXGI_FORMAT_B8G8R8A8_UNORM.0 as u64,
            width: self.width,
            height: self.height,
            _marker: PhantomData,
        })
    }

    /// Read the completed target, including the outer base-cache blit, as tightly packed BGRA.
    pub(crate) fn pixels(&self) -> Vec<u8> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            self.texture.GetDesc(&mut desc);
        }
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut staging))
                .unwrap();
        }
        let staging: ID3D11Texture2D = staging.unwrap();
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context.CopyResource(&staging, &self.texture);
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .unwrap();
        }
        let mut pixels = Vec::with_capacity((self.width * self.height * 4) as usize);
        for y in 0..self.height as usize {
            let row = unsafe {
                std::slice::from_raw_parts(
                    (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                    self.width as usize * 4,
                )
            };
            pixels.extend_from_slice(row);
        }
        unsafe {
            self.context.Unmap(&staging, 0);
        }
        pixels
    }

}
