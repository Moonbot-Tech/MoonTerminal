//! Coin icons from `assets/coins/{symbol}.png`: 32x32 color images from the
//! spothq/cryptocurrency-icons set under CC0; see `assets/coins/README.md`.
//! The full set is embedded in the binary with `include_dir`, while a file under the current working
//! directory's `assets/coins` or beside the executable takes priority so new icons can be supplied
//! without rebuilding. A global lazy symbol cache decodes each PNG and converts it to the BGRA
//! `RenderImage` layout expected by GPUI only once. Missing icons are also cached as `None` to avoid
//! repeated disk reads; callers render the coin without an icon.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use gpui::RenderImage;
use include_dir::{Dir, include_dir};

/// Embedded icon set containing all PNG files under `assets/coins`, approximately 412 KB.
static EMBEDDED: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/coins");

/// Locate the `assets/coins` override directory through the shared asset resolver.
fn coins_dir() -> PathBuf {
    super::asset_dir("coins")
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
    super::render_image_from_png(&bytes)
}

/// Return the lazily cached coin icon for a symbol such as `"BTC"`, `"usdt"`, or `"1000PEPE"`.
///
/// Args:
///     symbol: Coin or futures symbol to normalize and look up.
///
/// Returns:
///     The cached image, or `None` for an empty key or a missing or invalid PNG.
pub fn coin_icon(symbol: &str) -> Option<Arc<RenderImage>> {
    static CACHE: OnceLock<super::TextureCache<String>> = OnceLock::new();
    let key = icon_key(symbol);
    if key.is_empty() {
        return None;
    }
    super::cached(&CACHE, key, |key| load(key))
}
