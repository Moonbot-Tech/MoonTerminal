//! Windows capture of the chart slot, straight off the composited desktop.
//!
//! The desktop DC is read rather than the chart's own render target because the render target is
//! not finished when the chart can reach it. The chart draws through GPUI's `gpu_canvas` own pass,
//! and the fork submits every canvas's TEXT after every canvas's `draw`
//! (`moon-gpui-windows/src/directx_renderer.rs::draw`): the coin caption, the order-book numbers
//! and the axis labels all land on the back buffer after the last callback the chart is given, and
//! after `Present` a flip-model back buffer holds nothing defined. A readback from inside the own
//! pass would therefore return a chart with no writing on it — precisely the part the shot exists
//! to carry. There is no post-present hook, and adding one means changing MoonUI.
//!
//! What this costs instead: the capture is WYSIWYG. A tooltip, popover or foreign window over the
//! chart is in the picture, which is usually what the user wants and occasionally is not. A
//! minimized window is refused outright rather than captured as garbage.

use anyhow::{Context as _, bail};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, ClientToScreen,
    CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
    GetDIBits, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::IsIconic;

use super::rect::ShotRect;

/// Bytes for one captured rectangle, in the layout a `CF_DIB` body already wants.
pub(crate) struct DibImage {
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Rows BOTTOM-UP, three bytes per pixel in BGR order, each row padded to a 4-byte boundary.
    /// This is the DIB pixel array verbatim: it goes on the clipboard untouched, and
    /// [`Self::to_rgb_top_down`] is what turns it into something an encoder will take.
    pub(super) rows: Vec<u8>,
}

impl DibImage {
    /// Bytes per padded row.
    ///
    /// Returns:
    ///     The 4-byte-aligned length of one 24-bpp row.
    pub(super) fn stride(&self) -> usize {
        dib_stride(self.width)
    }

    /// Whether `rows` actually holds the picture `width` and `height` claim.
    ///
    /// The fields are plain and public within this module, so nothing in the type stops a producer
    /// from building a `DibImage` whose buffer disagrees with its dimensions — and there are two
    /// producers now, [`read_dib`] and [`rgb_to_dib`], where there used to be one. That
    /// disagreement is not a Rust-side bug: `rows.as_ptr()` is handed to `SetDIBitsToDevice`
    /// alongside a `BITMAPINFO` derived from the SAME width and height, so GDI reads exactly as
    /// many bytes as the header describes and a short buffer is an out-of-bounds read inside the
    /// OS. Checked at every FFI hand-off rather than trusted, because a documented invariant is
    /// only as good as the next person who adds a third producer.
    ///
    /// Returns:
    ///     `true` when the buffer length matches the padded stride times the height.
    pub(super) fn is_consistent(&self) -> bool {
        self.rows.len() == self.stride().saturating_mul(self.height as usize)
    }

    /// Repack to the top-down, unpadded, RGB form every image encoder expects.
    ///
    /// Three separate differences, all of them silent if missed: DIB rows run bottom-up, they are
    /// padded to a DWORD, and the channel order is BGR. Feeding the raw buffer to an encoder
    /// yields an upside-down, red-and-blue-swapped, progressively sheared image, and none of that
    /// is a compile error.
    ///
    /// Returns:
    ///     Top-down RGB bytes without DIB row padding.
    pub(crate) fn to_rgb_top_down(&self) -> Vec<u8> {
        let row_bytes = self.width as usize * 3;
        // Filled by `extend` rather than zeroed first: every byte is overwritten anyway, so a
        // `vec![0; ..]` here would memset the whole picture for nothing.
        let mut out = Vec::with_capacity(row_bytes * self.height as usize);
        // `.rev()` IS the bottom-up flip - a DIB stores the picture's LAST row first. The slice to
        // `row_bytes` drops the row's DWORD padding, and the triple swap is BGR -> RGB.
        for row in self.rows.chunks_exact(self.stride()).rev() {
            out.extend(
                row[..row_bytes]
                    .chunks_exact(3)
                    .flat_map(|px| [px[2], px[1], px[0]]),
            );
        }
        out
    }
}

/// Rebuild a DIB pixel array from top-down, unpadded RGB bytes.
///
/// The exact inverse of [`DibImage::to_rgb_top_down`], and it exists because the shot's
/// composition pass works in the encoder's layout while the clipboard's `CF_DIB` body must be
/// handed back in the DIB's. The same three differences apply in this direction — rows go
/// BOTTOM-UP, each row is padded to a DWORD, and the channel order is BGR — and every one of them
/// is as silent when missed here as it is there: the result still encodes, still pastes, and is
/// upside down, sheared, or red-and-blue-swapped.
///
/// Args:
///     width: Picture width in pixels.
///     height: Picture height in pixels.
///     rgb: `width * height * 3` bytes, top-down, RGB.
///
/// Returns:
///     The same picture in DIB layout, or `None` when `rgb` does not match the dimensions given -
///     a mismatch there would otherwise be read as a shear rather than as the bug it is.
pub(super) fn rgb_to_dib(width: u32, height: u32, rgb: &[u8]) -> Option<DibImage> {
    let row_bytes = width as usize * 3;
    if width == 0 || height == 0 || rgb.len() != row_bytes * height as usize {
        return None;
    }
    let stride = dib_stride(width);
    // Zeroed rather than `extend`ed, because the PADDING bytes at the end of every row are never
    // written below and must still be defined: they travel to the clipboard inside the CF_DIB body.
    let mut rows = vec![0u8; stride * height as usize];
    for (source, target) in rgb
        .chunks_exact(row_bytes)
        .rev()
        .zip(rows.chunks_exact_mut(stride))
    {
        for (px, out) in source.chunks_exact(3).zip(target.chunks_exact_mut(3)) {
            out[0] = px[2];
            out[1] = px[1];
            out[2] = px[0];
        }
    }
    Some(DibImage {
        width,
        height,
        rows,
    })
}

/// Padded row length for a 24-bpp DIB: three bytes per pixel, rounded up to a 4-byte boundary.
///
/// Args:
///     width: Image width in pixels.
///
/// Returns:
///     The aligned row length in bytes.
pub(super) fn dib_stride(width: u32) -> usize {
    (width as usize * 3).next_multiple_of(4)
}

/// Releases a screen DC obtained from `GetDC`.
pub(super) struct ScreenDc(pub(super) HDC);

impl Drop for ScreenDc {
    fn drop(&mut self) {
        unsafe { ReleaseDC(None, self.0) };
    }
}

/// Deletes a memory DC created by `CreateCompatibleDC`. A different call from `ScreenDc`'s on
/// purpose: `ReleaseDC` on a memory DC leaks it and `DeleteDC` on a window DC is an error.
pub(super) struct MemoryDc(pub(super) HDC);

impl Drop for MemoryDc {
    fn drop(&mut self) {
        // A failed delete leaks one DC and nothing else; there is no recovery from a destructor.
        let _ = unsafe { DeleteDC(self.0) }.ok();
    }
}

/// Deletes a bitmap created by `CreateCompatibleBitmap`.
pub(super) struct Bitmap(pub(super) HBITMAP);

impl Drop for Bitmap {
    fn drop(&mut self) {
        let _ = unsafe { DeleteObject(self.0.into()) }.ok();
    }
}

/// Copy one client-relative rectangle of `hwnd` off the composited desktop.
///
/// The rectangle arrives in the window's PHYSICAL client pixels and is converted to screen
/// coordinates here. That conversion is only sound because the process is per-monitor DPI aware
/// (GPUI sets that on Windows): the chart's device pixels are the window's physical client pixels,
/// `ClientToScreen` answers in physical virtual-screen coordinates, and the desktop DC spans that
/// same virtual-screen space — including the negative coordinates a monitor left of or above the
/// primary one produces. Under a DPI-virtualized process all three of those would be false.
///
/// Args:
///     hwnd: The OS window the chart is drawn in.
///     rect: The chart slot, in physical pixels relative to that window's client area.
///
/// Returns:
///     The captured pixels, or an error naming the step that failed.
pub(crate) fn capture_client_rect(hwnd: HWND, rect: ShotRect) -> anyhow::Result<DibImage> {
    // Nothing on screen to read. Refused rather than captured: a minimized window's client area
    // maps to some other window's pixels, which would be copied without complaint.
    if unsafe { IsIconic(hwnd) }.as_bool() {
        bail!("window is minimized");
    }
    let width = i32::try_from(rect.width).context("capture width does not fit in i32")?;
    let height = i32::try_from(rect.height).context("capture height does not fit in i32")?;

    let mut origin = POINT {
        x: rect.x,
        y: rect.y,
    };
    unsafe { ClientToScreen(hwnd, &mut origin) }
        .ok()
        .context("ClientToScreen")?;

    let screen = unsafe { GetDC(None) };
    if screen.is_invalid() {
        bail!("GetDC(None) returned no desktop DC");
    }
    let screen = ScreenDc(screen);

    let memory = unsafe { CreateCompatibleDC(Some(screen.0)) };
    if memory.is_invalid() {
        bail!("CreateCompatibleDC failed");
    }
    let memory = MemoryDc(memory);

    // Compatible with the SCREEN DC, never the memory DC: a fresh memory DC holds a 1x1 monochrome
    // bitmap, so asking it for a compatible bitmap yields a monochrome one and the capture comes
    // back black and white.
    let bitmap = unsafe { CreateCompatibleBitmap(screen.0, width, height) };
    if bitmap.is_invalid() {
        bail!("CreateCompatibleBitmap failed for {width}x{height}");
    }
    let bitmap = Bitmap(bitmap);

    // The bitmap must be SELECTED for `BitBlt` to draw into it and DESELECTED before `GetDIBits`,
    // which documents that its bitmap may not be selected into any DC. Both halves are load-bearing
    // and the failure is quiet either way: an unselected blit paints the memory DC's default 1x1
    // bitmap, and a still-selected `GetDIBits` returns zero rows.
    let previous = unsafe { SelectObject(memory.0, bitmap.0.into()) };
    let blit = unsafe {
        BitBlt(
            memory.0,
            0,
            0,
            width,
            height,
            Some(screen.0),
            origin.x,
            origin.y,
            // CAPTUREBLT so layered windows - tooltips, popovers, the chart's own overlays - are
            // included rather than punched out of the picture.
            SRCCOPY | CAPTUREBLT,
        )
    };
    // Restored before the error is raised, so a failed blit does not leave the bitmap selected into
    // a DC we are about to delete.
    restore_selection(memory.0, previous);
    blit.context("BitBlt from the desktop DC")?;

    read_dib(screen.0, bitmap.0, rect.width, rect.height)
}

/// Copy a GDI bitmap's pixels out as a `CF_DIB`-shaped array.
///
/// The ONE place `GetDIBits` is asked for this shape, and deliberately so: the shot has two
/// producers of a bitmap - the desktop capture above and the strip composition in
/// `super::paint_win` - and their outputs both end up on the clipboard, where a `CF_DIB` body must
/// agree exactly with the header `dib_header_bytes` derives. Two copies of these fields could
/// drift; one cannot.
///
/// The bitmap must NOT be selected into any DC when this is called - `GetDIBits` documents that,
/// and a still-selected bitmap returns zero rows rather than an error.
///
/// Args:
///     screen: A DC compatible with the bitmap; only used to describe the format.
///     bitmap: The bitmap to read.
///     width: Its width in pixels.
///     height: Its height in pixels.
///
/// Returns:
///     The pixels in DIB layout, or an error naming how many scan lines actually arrived.
pub(super) fn read_dib(
    screen: HDC,
    bitmap: HBITMAP,
    width: u32,
    height: u32,
) -> anyhow::Result<DibImage> {
    let signed_height = i32::try_from(height).context("bitmap height does not fit in i32")?;
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: u32::try_from(std::mem::size_of::<BITMAPINFOHEADER>())
                .expect("BITMAPINFOHEADER fits in u32"),
            biWidth: i32::try_from(width).context("bitmap width does not fit in i32")?,
            // POSITIVE height asks for the DIB's own bottom-up row order, which is what a CF_DIB
            // body must be. `DibImage::to_rgb_top_down` flips it for the encoder.
            biHeight: signed_height,
            biPlanes: 1,
            // 24 bpp on purpose. Alpha in a BI_RGB DIB is undefined, and consumers that read it
            // anyway see zero and paste a fully transparent - usually black - rectangle. A screen
            // capture is opaque, so the fourth channel carries nothing worth that risk.
            biBitCount: 24,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let stride = dib_stride(width);
    let mut rows = vec![0u8; stride * height as usize];
    let copied = unsafe {
        GetDIBits(
            screen,
            bitmap,
            0,
            height,
            Some(rows.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    if copied != signed_height {
        bail!("GetDIBits copied {copied} of {signed_height} scan lines");
    }

    Ok(DibImage {
        width,
        height,
        rows,
    })
}

/// Put back whatever the memory DC held before the bitmap went in.
///
/// `SelectObject` answers a null handle only when it never had a previous object to give back, so
/// a null is nothing to restore rather than a failure worth reporting.
///
/// Args:
///     dc: Memory device context that previously selected `previous`.
///     previous: Object replaced by the capture bitmap.
///
/// Returns:
///     Nothing; restores `previous` when it is valid.
pub(super) fn restore_selection(dc: HDC, previous: HGDIOBJ) {
    if !previous.is_invalid() {
        unsafe { SelectObject(dc, previous) };
    }
}

/// The bitmap header a `CF_DIB` body is prefixed with, as raw bytes.
///
/// `CF_DIB` is a `BITMAPINFOHEADER` immediately followed by the pixel array — a `.bmp` file
/// without its 14-byte file header. Built here rather than in the clipboard module because the
/// fields have to agree exactly with the ones `GetDIBits` was asked for above.
///
/// Args:
///     image: Captured 24-bpp DIB pixels whose dimensions and row length define the header.
///
/// Returns:
///     Native-endian bytes of the header that prefixes a `CF_DIB` payload.
pub(super) fn dib_header_bytes(image: &DibImage) -> Vec<u8> {
    let header = BITMAPINFOHEADER {
        biSize: u32::try_from(std::mem::size_of::<BITMAPINFOHEADER>())
            .expect("BITMAPINFOHEADER fits in u32"),
        // Plain casts, and that is safe HERE rather than everywhere: every caller validates the
        // image with `DibImage::is_consistent` and bounds both dimensions to `i32` before staging
        // a `CF_DIB` body, so a value that could truncate never reaches this function. The check
        // lives at the hand-off because that is where a bad value would become an OS-side read;
        // repeating it here would only move the question, not answer it.
        biWidth: image.width as i32,
        biHeight: image.height as i32,
        biPlanes: 1,
        biBitCount: 24,
        biCompression: BI_RGB.0,
        biSizeImage: u32::try_from(image.rows.len()).unwrap_or(0),
        ..Default::default()
    };
    // SAFETY: `BITMAPINFOHEADER` is `#[repr(C)]` and holds only integers, so every byte of it is
    // initialized and reading it as bytes has no padding or provenance hazard.
    let bytes = unsafe {
        std::slice::from_raw_parts(
            std::ptr::from_ref(&header).cast::<u8>(),
            std::mem::size_of::<BITMAPINFOHEADER>(),
        )
    };
    bytes.to_vec()
}

#[cfg(test)]
mod tests;
