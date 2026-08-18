//! Publishing one captured chart under both clipboard formats that matter on Windows.
//!
//! GPUI can already put an image on the clipboard, and it is deliberately not used here: its
//! Windows writer (`moon-gpui-windows/src/clipboard.rs::write_image`) publishes ONLY the registered
//! `"PNG"` format. Chromium-based applications read that one; Paint, Word, Excel and most native
//! Win32 consumers read `CF_DIB` and would find the clipboard empty. Nor can GPUI's write be
//! topped up afterwards: `SetClipboardData` requires clipboard ownership, ownership is established
//! by `EmptyClipboard`, and GPUI has closed the clipboard before it returns. One writer has to
//! publish both, so it is this one.
//!
//! # TEMPORARY EXCEPTION to the MoonUI boundary
//!
//! `docs/ARCHITECTURE.md` -> "UI Components" says a missing MoonUI hook is ADDED to MoonUI rather
//! than replaced by local code here, and allows a temporary exception marked with its reason and a
//! removal plan. This is that marker.
//!
//! **Reason.** The gap is one clipboard FORMAT, not a UI component, and closing it properly means a
//! change to `moon-gpui-windows` — a different repository, deliberately outside the scope this
//! feature was built in.
//!
//! **Removal plan.** Teach `moon-gpui-windows/src/clipboard.rs::write_image` to publish `CF_DIB`
//! alongside the registered `"PNG"` format it already writes (its `read_image` reads `CF_DIB`
//! already, so the asymmetry is the actual defect). Once that ships, this whole module deletes and
//! `capture_windows` calls `cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
//! ImageFormat::Png, png)))` instead — which also makes the shot portable the moment any other
//! platform grows a capture path.

use anyhow::{Context as _, bail};
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_DIB;
use windows::core::Owned;

use super::win::{DibImage, dib_header_bytes};

/// Holds the clipboard open for as long as it is alive.
struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        if let Err(error) = unsafe { CloseClipboard() } {
            log::warn!("chart shot: CloseClipboard failed: {error}");
        }
    }
}

/// Publish the captured chart as `CF_DIB` and as a PNG under the registered `"PNG"` format.
///
/// `owner` must be a real window of this process. GPUI opens the clipboard with a null owner, and
/// that is documented to make `EmptyClipboard` leave the owner null and every following
/// `SetClipboardData` fail — a deviation from the code next door that is deliberate, not an
/// oversight. The chart shot always has a window to name, so it names it.
///
/// Args:
///     owner: The window that takes clipboard ownership.
///     image: The captured pixels, in DIB layout.
///
/// Returns:
///     `Ok(())` once both formats are on the clipboard.
pub(super) fn publish(owner: HWND, image: &DibImage) -> anyhow::Result<()> {
    // Encoded BEFORE the clipboard is opened. Holding the global clipboard lock across a PNG
    // encode would stall every other application that touches the clipboard meanwhile, and the
    // encode is by far the slowest step here.
    let png = encode_png(image).context("encoding the capture as PNG")?;

    let dib_header = dib_header_bytes(image);

    // BOTH blocks are allocated and filled BEFORE the clipboard is opened, and that ordering is the
    // whole point rather than a tidiness preference. `EmptyClipboard` destroys whatever the user had
    // copied; allocating afterwards means an allocation failure leaves them with neither their old
    // clipboard nor a picture. Everything that can fail cheaply therefore fails while their
    // clipboard is still intact, and the ownership window below holds only the two calls that
    // cannot be moved out of it.
    // Staged from PARTS so the capture - by far the largest buffer here - is copied straight into
    // the global block instead of first into a header-plus-pixels Vec that exists only to be copied
    // again. A CF_DIB body is exactly the header followed by the pixel array.
    let dib_block = stage(&[&dib_header, &image.rows]).context("staging CF_DIB")?;
    let png_block = stage(&[&png]).context("staging the PNG format")?;

    unsafe { OpenClipboard(Some(owner)) }.context("OpenClipboard")?;
    let _guard = ClipboardGuard;
    unsafe { EmptyClipboard() }.context("EmptyClipboard")?;

    // CF_DIB first: it is the format the widest set of applications reads, so even if the second
    // hand-off failed the clipboard would still hold a usable picture.
    hand_over(dib_block, CF_DIB.0 as u32).context("publishing CF_DIB")?;
    hand_over(png_block, png_format()).context("publishing the PNG format")?;
    Ok(())
}

/// The registered `"PNG"` clipboard format id.
///
/// Deliberately the same NAME GPUI registers: `RegisterClipboardFormatW` returns the same id for
/// the same name process-wide and system-wide, so GPUI's own reader recognizes what we publish and
/// an in-application paste round-trips.
///
/// Returns:
///     The registered format identifier for the case-insensitive `PNG` name.
fn png_format() -> u32 {
    unsafe { RegisterClipboardFormatW(windows::core::w!("PNG")) }
}

/// Encode the capture as PNG in memory. Nothing reaches the disk anywhere in the shot.
///
/// Args:
///     image: Captured DIB pixels to convert to top-down RGB PNG data.
///
/// Returns:
///     Encoded PNG bytes, or an encoding error.
fn encode_png(image: &DibImage) -> anyhow::Result<Vec<u8>> {
    use image::ImageEncoder as _;

    let rgb = image.to_rgb_top_down();
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png).write_image(
        &rgb,
        image.width,
        image.height,
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(png)
}

/// Concatenate `parts` into a moveable global block the clipboard could take.
///
/// Called with the clipboard CLOSED. The returned `Owned` frees the block on drop, so every failure
/// from here up to the hand-off costs nothing but the allocation.
///
/// Args:
///     parts: Byte slices concatenated into one clipboard-owned global-memory block.
///
/// Returns:
///     A moveable global-memory allocation holding all `parts`, or an allocation error.
fn stage(parts: &[&[u8]]) -> anyhow::Result<Owned<HGLOBAL>> {
    let total: usize = parts.iter().map(|part| part.len()).sum();
    if total == 0 {
        bail!("refusing to publish an empty clipboard buffer");
    }
    unsafe {
        let global = Owned::new(GlobalAlloc(GMEM_MOVEABLE, total)?);
        let base = GlobalLock(*global);
        if base.is_null() {
            bail!("GlobalLock returned null");
        }
        let mut at = base.cast::<u8>();
        for part in parts {
            std::ptr::copy_nonoverlapping(part.as_ptr(), at, part.len());
            at = at.add(part.len());
        }
        GlobalUnlock(*global).ok();
        Ok(global)
    }
}

/// Give a staged block to the clipboard under `format`.
///
/// The clipboard takes OWNERSHIP on success, so the handle must not be freed afterwards — hence the
/// `forget`, which runs only once `SetClipboardData` has actually accepted it. On the failure path
/// the `Owned` still drops and the block is freed, so a refused hand-off leaks nothing.
///
/// Requires the clipboard to be open and owned by this process.
///
/// Args:
///     block: Moveable global-memory data handed to the clipboard on success.
///     format: Standard or registered clipboard format identifier for `block`.
///
/// Returns:
///     `Ok(())` after ownership transfers, or an error while `block` remains owned locally.
fn hand_over(block: Owned<HGLOBAL>, format: u32) -> anyhow::Result<()> {
    unsafe {
        SetClipboardData(format, Some(HANDLE(block.0)))?;
        std::mem::forget(block);
    }
    Ok(())
}
