//! One-shot native own-pass image readback for the frozen FireTest recovery stage.
//! No GPU copy or allocation occurs unless the diagnostic explicitly requests a capture.

use std::sync::Arc;
use windows::{Win32::Graphics::Direct3D11::*, core::Interface as _};

/// The chart's completed native plot pixels, excluding the later GPUI text/chrome pass.
#[derive(Debug)]
pub(crate) struct RecoveryImage {
    pub(crate) generation: u64,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: Vec<u8>,
}

impl RecoveryImage {
    /// Reject an empty/unpainted baseline; matching two blank images is not recovery evidence.
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.width < 100
            || self.height < 50
            || self.pixels.len() != (self.width * self.height * 4) as usize
        {
            return Err(format!(
                "recovery image has invalid size {}x{}",
                self.width, self.height
            ));
        }
        let first = &self.pixels[..4];
        let painted = self
            .pixels
            .chunks_exact(4)
            .filter(|pixel| *pixel != first)
            .count();
        if painted < (self.width * self.height / 1000) as usize {
            return Err(format!(
                "recovery plot is blank: {painted} differing pixels"
            ));
        }
        Ok(())
    }

    /// Compare every own-pass pixel, so losing even a thin figure cannot hide behind tick counts.
    pub(crate) fn compare(&self, after: &Self) -> Result<(), String> {
        if (self.width, self.height) != (after.width, after.height) {
            return Err("recovery plot geometry changed".into());
        }
        let changed = self
            .pixels
            .chunks_exact(4)
            .zip(after.pixels.chunks_exact(4))
            .filter(|(before, after)| before != after)
            .count();
        if self.pixels.len() != after.pixels.len() || changed != 0 {
            return Err(format!(
                "recovery own-pass image changed {changed}/{} pixels",
                self.pixels.len() / 4
            ));
        }
        Ok(())
    }
}

/// A request is consumed only after the native draw has submitted all chart layers.
pub(in crate::chartdx) enum Capture {
    Requested,
    Ready(Result<Arc<RecoveryImage>, String>),
    CheckClip,
    ClipChecked(Result<(), String>),
}

/// Read a clipped chart rectangle from the actual draw target using a staging texture.
/// The owning UI thread serializes this with draws; COM handles never escape the callback.
pub(in crate::chartdx) fn capture(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    target: &ID3D11RenderTargetView,
    clip: [f32; 4],
    generation: u64,
) -> anyhow::Result<RecoveryImage> {
    let source: ID3D11Texture2D = unsafe { target.GetResource()? }.cast()?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe {
        source.GetDesc(&mut desc);
    }
    anyhow::ensure!(
        desc.Format == windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
        "unsupported recovery target format {:?}",
        desc.Format
    );
    let left = clip[0].floor().max(0.) as u32;
    let top = clip[1].floor().max(0.) as u32;
    let right = (clip[2].ceil().max(0.) as u32).min(desc.Width);
    let bottom = (clip[3].ceil().max(0.) as u32).min(desc.Height);
    anyhow::ensure!(
        right > left && bottom > top && desc.SampleDesc.Count == 1,
        "invalid recovery target crop"
    );
    let width = right - left;
    let height = bottom - top;
    desc.Width = width;
    desc.Height = height;
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    desc.MiscFlags = 0;
    let mut staging = None;
    unsafe {
        device.CreateTexture2D(&desc, None, Some(&mut staging))?;
    }
    let staging = staging.ok_or_else(|| anyhow::anyhow!("missing recovery staging texture"))?;
    let source_box = D3D11_BOX {
        left,
        top,
        front: 0,
        right,
        bottom,
        back: 1,
    };
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        context.CopySubresourceRegion(&staging, 0, 0, 0, 0, &source, 0, Some(&source_box));
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
    }
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height as usize {
        let row = unsafe {
            std::slice::from_raw_parts(
                (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                width as usize * 4,
            )
        };
        pixels.extend_from_slice(row);
    }
    unsafe {
        context.Unmap(&staging, 0);
    }
    Ok(RecoveryImage {
        generation,
        width,
        height,
        pixels,
    })
}

#[cfg(test)]
mod tests;
