//! Brand-coloured exchange logos from `assets/exchanges/{slug}.svg`, rasterized for GPUI.
//!
//! GPUI's `svg()` element is an alpha mask painted with a single `text_color`, so a two-colour
//! brand tile drawn through it collapses into one flat silhouette. These logos therefore take the
//! same route as the coin icons: decode once, hand GPUI a `RenderImage`, and draw it with `img()`.
//! The difference is only the decoder — resvg rasterizes the vector at a fixed generous size and
//! GPUI scales it down to the row height, which keeps the cache independent of display scale.
//!
//! The set is embedded with `include_dir` like `coin_icons.rs`, and a file under the working
//! directory or beside the executable takes priority so a logo can be replaced without rebuilding.
//! An exchange with no logo simply renders without one — never a placeholder glyph pretending to
//! be a brand.
//!
//! WHICH logo a core gets is not decided here: `moon_core::venue` resolves the core's platform code
//! to a [`Brand`], and this module only rasterizes that brand's file. Matching on the name a core
//! reports is what used to leave Binance COIN-M (`Binance Quarterly`) unbranded.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use gpui::RenderImage;
use include_dir::{Dir, include_dir};
use moon_core::venue::Brand;

/// Embedded logo set: every SVG under `assets/exchanges`, a few kilobytes in total.
static EMBEDDED: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/exchanges");

/// Square edge every logo is rasterized at, in texture pixels.
///
/// One size for all call sites: the logos are drawn at roughly 12–18 logical pixels, so 48 covers
/// a 200% display without a per-scale cache, and GPUI's own sampling handles the way down.
const RASTER_PX: u32 = 48;

/// Resolve the override directory once.
///
/// `asset_dir` stats the working directory and possibly the executable's, and `load` runs per
/// brand; probing the filesystem seven times for one unchanging answer is pure syscall noise.
fn logos_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| super::asset_dir("exchanges"))
}

/// Rasterize one logo stem into the straight-alpha BGRA layout GPUI's `img()` expects.
///
/// Args:
///     slug: Logo file stem from [`Brand::slug`].
///
/// Returns:
///     The decoded texture, or `None` when the file is missing or unparsable.
fn load(slug: &str) -> Option<Arc<RenderImage>> {
    let file = format!("{slug}.svg");
    let path = logos_dir().join(&file);
    let bytes: Vec<u8> = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            // A MISSING override is the normal case and says nothing. Anything else — a locked or
            // unreadable file the user put there on purpose — is worth one line, or the override
            // silently does nothing and the embedded logo makes it look like it worked.
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!("exchange logo {}: {error}", path.display());
            }
            EMBEDDED.get_file(&file)?.contents().to_vec()
        }
    };
    let tree = resvg::usvg::Tree::from_data(&bytes, &resvg::usvg::Options::default())
        .map_err(|error| log::warn!("exchange logo {slug}: unparsable SVG: {error}"))
        .ok()?;
    let size = tree.size();
    let longest = size.width().max(size.height());
    if longest <= 0.0 {
        return None;
    }
    // Fit the longest edge and CENTRE the rest. The shipped logos are square, but a replacement
    // dropped into `assets/exchanges` need not be, and scaling one axis to fill would distort a
    // brand mark while anchoring it top-left would hang it off centre in the row.
    let scale = RASTER_PX as f32 / longest;
    let offset_x = (RASTER_PX as f32 - size.width() * scale) * 0.5;
    let offset_y = (RASTER_PX as f32 - size.height() * scale) * 0.5;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(RASTER_PX, RASTER_PX)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_translate(offset_x, offset_y).pre_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // resvg paints premultiplied RGBA; GPUI takes straight-alpha BGRA, the same layout the PNG
    // loaders produce. Demultiplying here rather than in the renderer keeps one convention.
    let mut bgra = Vec::with_capacity((RASTER_PX * RASTER_PX * 4) as usize);
    for pixel in pixmap.pixels() {
        let color = pixel.demultiply();
        bgra.extend_from_slice(&[color.blue(), color.green(), color.red(), color.alpha()]);
    }
    super::render_image_bgra(RASTER_PX, RASTER_PX, bgra)
}

/// Return the lazily rasterized logo for one brand.
///
/// Every decode is cached, including the ones that produced nothing, so a brand is rasterized at
/// most once per process.
///
/// Args:
///     brand: Brand resolved from a core's platform code by `moon_core::venue`.
///
/// Returns:
///     The cached texture, or `None` when the file is missing or unreadable.
pub(crate) fn exchange_logo(brand: Brand) -> Option<Arc<RenderImage>> {
    static CACHE: OnceLock<super::TextureCache<&'static str>> = OnceLock::new();
    super::cached(&CACHE, brand.slug(), |slug| load(slug))
}

/// Decode every shipped logo, for a caller that will run this OFF the render path.
///
/// Without it the first frame of a logo-drawing surface pays for the disk read and the raster
/// inline — small, but squarely inside the frame loop. Deliberately a plain blocking function and
/// not an `async fn` with no `await` in it: the obligation to keep it off the render thread belongs
/// to the caller, and a future would disguise that. A process-wide gate makes concurrent callers
/// share one blocking flight; each waiting Shell can still publish its own ready edge afterward.
pub(crate) fn prewarm() {
    static PREWARMED: OnceLock<()> = OnceLock::new();
    prewarm_once(&PREWARMED, || {
        for brand in Brand::ALL {
            let _ = exchange_logo(brand);
        }
    });
}

/// Run one blocking prewarm initializer at most once for a shared flight gate.
///
/// Args:
///     gate: Process-shared completion and contention gate.
///     warm: Blocking initializer invoked only by the winning caller.
///
/// Returns:
///     Nothing; concurrent callers block until the winning initializer completes.
fn prewarm_once(gate: &OnceLock<()>, warm: impl FnOnce()) {
    gate.get_or_init(|| warm());
}

#[cfg(test)]
mod tests;
