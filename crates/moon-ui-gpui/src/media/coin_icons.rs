//! Coin icons from `assets/coins/{symbol}.png`: 32x32 color images from the
//! spothq/cryptocurrency-icons set under CC0; see `assets/coins/README.md`.
//! The full set is embedded in the binary with `include_dir`, while a file under the current working
//! directory's `assets/coins` or beside the executable takes priority so new icons can be supplied
//! without rebuilding. A global lazy symbol cache decodes each PNG and converts it to the BGRA
//! `RenderImage` layout expected by GPUI only once. Missing icons are also cached as `None` to avoid
//! repeated disk reads; callers render the coin without an icon.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use gpui::RenderImage;
use image::{Frame, ImageBuffer, Rgba};
use include_dir::{Dir, include_dir};

/// Embedded icon set containing all PNG files under `assets/coins`, approximately 412 KB.
static EMBEDDED: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/coins");

/// Locate `assets/coins` under the current working directory, then beside the executable.
///
/// Returns:
///     The first existing `assets/coins` directory, or the relative path when neither exists.
fn coins_dir() -> PathBuf {
    let rel = PathBuf::from("assets/coins");
    if rel.is_dir() {
        return rel;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("assets/coins");
            if p.is_dir() {
                return p;
            }
        }
    }
    rel
}

/// Build a lowercase icon filename key and repeatedly remove a nonempty literal `1000` prefix.
///
/// This maps a futures symbol such as `1000PEPE` to `pepe`; it does not parse arbitrary numeric
/// multipliers.
///
/// Args:
///     symbol: Coin or futures symbol supplied by the caller.
///
/// Returns:
///     The trimmed, lowercase key after literal `1000` prefixes are removed.
fn icon_key(symbol: &str) -> String {
    let mut s = symbol.trim().to_ascii_lowercase();
    while let Some(rest) = s.strip_prefix("1000") {
        if rest.is_empty() {
            break;
        }
        s = rest.to_string();
    }
    s
}

/// Load `{key}.png` into a `RenderImage`, preferring a disk override over the embedded set.
///
/// The decoded RGBA red and blue channels are swapped to produce the BGRA layout expected by GPUI.
///
/// Args:
///     key: Normalized lowercase icon filename stem.
///
/// Returns:
///     The decoded image, or `None` when no file exists or decoding fails.
fn load(key: &str) -> Option<Arc<RenderImage>> {
    let file = format!("{key}.png");
    let bytes: Vec<u8> = match std::fs::read(coins_dir().join(&file)) {
        Ok(b) => b,
        Err(_) => EMBEDDED.get_file(&file)?.contents().to_vec(),
    };
    let mut img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    for px in img.pixels_mut() {
        px.0.swap(0, 2);
    }
    let (w, h) = img.dimensions();
    let buf = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, img.into_raw())?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(buf)])))
}

/// Return the lazily cached coin icon for a symbol such as `"BTC"`, `"usdt"`, or `"1000PEPE"`.
///
/// Args:
///     symbol: Coin or futures symbol to normalize and look up.
///
/// Returns:
///     The cached image, or `None` for an empty key or a missing or invalid PNG.
pub fn coin_icon(symbol: &str) -> Option<Arc<RenderImage>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<RenderImage>>>>> = OnceLock::new();
    let key = icon_key(symbol);
    if key.is_empty() {
        return None;
    }
    let mut map = CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some(v) = map.get(&key) {
        return v.clone();
    }
    let tex = load(&key);
    map.insert(key, tex.clone());
    tex
}
