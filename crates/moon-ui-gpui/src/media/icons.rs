//! GPUI port of the egui group-icon loader for `assets/icons/{id}.png`.
//! PNGs are decoded with `image`, converted to the BGRA layout expected by GPUI's `img(..)`, and
//! cached by ID. The base set is embedded in the binary with `include_dir`; a file in the selected
//! on-disk directory under the current working directory or beside the executable takes priority,
//! allowing icons to be added or replaced without rebuilding. Like `coin_icons.rs`, embedding keeps
//! the picker populated when a deployed installation has no `assets/icons` directory.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::RenderImage;
use include_dir::{Dir, include_dir};

/// Embedded group-icon set containing all PNG files under `assets/icons`, approximately 64 KB.
static EMBEDDED: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/icons");

/// Locate the `assets/icons` override directory through the shared asset resolver.
fn icons_dir() -> PathBuf {
    super::asset_dir("icons")
}

/// Parses the numeric stem of an exact lowercase `{id}.png` filename for either icon source.
fn id_from_png_name(name: &str) -> Option<u32> {
    name.strip_suffix(".png")?.parse::<u32>().ok()
}

/// Loads `{id}.png` into a BGRA `RenderImage`, preferring the selected disk directory.
/// A disk read failure falls back to the embedded file; a missing embedded file or decode failure
/// returns `None`.
fn load_render_image(id: u32) -> Option<Arc<RenderImage>> {
    let file = format!("{id}.png");
    let bytes: Vec<u8> = match std::fs::read(icons_dir().join(&file)) {
        Ok(b) => b,
        Err(_) => EMBEDDED.get_file(&file)?.contents().to_vec(),
    };
    super::render_image_from_png(&bytes)
}

/// Per-settings-window group-icon catalog and lazy cache keyed by numeric ID.
pub struct IconSet {
    /// Sorted, deduplicated IDs discovered across the embedded and disk sets; gaps are allowed.
    pub ids: Vec<u32>,
    cache: HashMap<u32, Option<Arc<RenderImage>>>,
}

impl IconSet {
    pub fn discover() -> Self {
        // Use the embedded set as the baseline and union it with disk IDs so installations can add
        // or override icons without rebuilding.
        let mut ids: Vec<u32> = EMBEDDED
            .files()
            .filter_map(|f| f.path().file_name()?.to_str().and_then(id_from_png_name))
            .collect();
        if let Ok(rd) = std::fs::read_dir(icons_dir()) {
            for e in rd.filter_map(|e| e.ok()) {
                if let Some(id) = e.file_name().to_str().and_then(id_from_png_name) {
                    ids.push(id);
                }
            }
        }
        ids.sort_unstable();
        ids.dedup();
        Self {
            ids,
            cache: HashMap::new(),
        }
    }

    /// Returns an icon by ID, loading it lazily and caching both success and failure. `None` means
    /// the file is absent or invalid.
    pub fn texture(&mut self, id: u32) -> Option<Arc<RenderImage>> {
        if let Some(c) = self.cache.get(&id) {
            return c.clone();
        }
        let tex = load_render_image(id);
        self.cache.insert(id, tex.clone());
        tex
    }
}

impl Default for IconSet {
    fn default() -> Self {
        Self::discover()
    }
}
