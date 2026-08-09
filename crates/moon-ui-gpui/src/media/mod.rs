//! Embedded-asset loaders: coin, exchange and UI icon textures, and sound playback.

use std::collections::HashMap;
use std::hash::Hash;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use gpui::RenderImage;
use image::{Frame, ImageBuffer, Rgba};

pub(crate) mod coin_icons;
pub(crate) mod exchange_logos;
pub(crate) mod icons;
pub(crate) mod sound;

/// Locate one `assets/<name>` directory: under the working directory first, then beside the
/// executable.
///
/// Every asset loader here ships its set embedded with `include_dir` and lets a file on disk take
/// priority, so a deployed installation can add or replace one without a rebuild. This is that
/// override rule, in one place — it used to be copied verbatim into three loaders, which meant the
/// beside-the-executable fallback had three definitions that could drift apart.
///
/// Args:
///     name: Directory name under `assets`, such as `"coins"`.
///
/// Returns:
///     The first existing directory, or the relative path when neither exists — callers treat a
///     failed read as "no override" and fall back to the embedded copy.
pub(crate) fn asset_dir(name: &str) -> PathBuf {
    let relative = PathBuf::from("assets").join(name);
    if relative.is_dir() {
        return relative;
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let beside_exe = dir.join("assets").join(name);
        if beside_exe.is_dir() {
            return beside_exe;
        }
    }
    relative
}

/// A process-wide texture cache: decoded images by key, including the decodes that produced nothing.
///
/// Caching the misses matters as much as caching the hits — a symbol with no icon must not probe
/// the disk again on every rendered row.
pub(crate) type TextureCache<K> = Mutex<HashMap<K, Option<Arc<RenderImage>>>>;

/// Return a cached texture, decoding it once on the first miss.
///
/// The decode runs OUTSIDE the lock. Holding a process-global mutex across a disk read and an image
/// decode blocks every other caller behind it, and these callers are render passes — that is a
/// stall on the frame loop, not a slow lookup. Two threads racing the same miss both decode and
/// then agree, which is the cheaper trade.
///
/// Args:
///     cache: The caller's own `OnceLock` cache slot.
///     key: Normalized cache key.
///     load: Decoder, run at most once per key in the common case.
///
/// Returns:
///     The cached texture, or `None` when the key has no usable image.
pub(crate) fn cached<K: Eq + Hash>(
    cache: &OnceLock<TextureCache<K>>,
    key: K,
    load: impl FnOnce(&K) -> Option<Arc<RenderImage>>,
) -> Option<Arc<RenderImage>> {
    let cache = cache.get_or_init(|| Mutex::new(HashMap::new()));
    // A poisoned lock means some other caller panicked mid-insert; the map itself is still a valid
    // cache, and refusing to draw an icon over it would be the worse answer.
    let lock = || {
        cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    };
    if let Some(hit) = lock().get(&key) {
        return hit.clone();
    }
    let texture = load(&key);
    lock().insert(key, texture.clone());
    texture
}

/// Wrap straight-alpha BGRA bytes as the single-frame image GPUI's `img()` draws.
///
/// GPUI takes BGRA, not RGBA. Every loader here converts to it, so the layout contract lives in one
/// place rather than being restated wherever an image is decoded.
///
/// Args:
///     width: Image width in pixels.
///     height: Image height in pixels.
///     bgra: `width * height * 4` bytes of straight-alpha BGRA.
///
/// Returns:
///     The image, or `None` when the buffer does not match the dimensions.
pub(crate) fn render_image_bgra(
    width: u32,
    height: u32,
    bgra: Vec<u8>,
) -> Option<Arc<RenderImage>> {
    let buffer = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(width, height, bgra)?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(buffer)])))
}

/// Decode a PNG into the layout [`render_image_bgra`] expects.
///
/// Args:
///     bytes: Encoded PNG.
///
/// Returns:
///     The decoded image, or `None` when the bytes are not a readable image.
pub(crate) fn render_image_from_png(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let mut image = image::load_from_memory(bytes).ok()?.to_rgba8();
    for pixel in image.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    let (width, height) = image.dimensions();
    render_image_bgra(width, height, image.into_raw())
}

#[cfg(test)]
mod tests;
