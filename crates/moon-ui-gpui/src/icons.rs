//! Иконки групп из `assets/icons/{id}.png` — порт egui `src/icons.rs` на gpui.
//! Грузит PNG (image crate) в `RenderImage` (BGRA, как ждёт gpui от `img(..)`),
//! кэширует по id. Каталог — рядом с cwd или с exe (как egui). DLL в рантайме не нужна.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::RenderImage;
use image::{Frame, ImageBuffer, Rgba};

/// Каталог иконок: `assets/icons` рядом с cwd, иначе рядом с exe.
fn icons_dir() -> PathBuf {
    let rel = PathBuf::from("assets/icons");
    if rel.is_dir() {
        return rel;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("assets/icons");
            if p.is_dir() {
                return p;
            }
        }
    }
    rel
}

/// Загрузить `{id}.png` в `RenderImage` (BGRA — gpui свопает R/B, как и для чарта).
fn load_render_image(id: u32) -> Option<Arc<RenderImage>> {
    let path = icons_dir().join(format!("{id}.png"));
    let bytes = std::fs::read(path).ok()?;
    let mut img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    // RGBA → BGRA: gpui RenderImage ждёт порядок BGRA (иначе R↔B свопаются).
    for px in img.pixels_mut() {
        px.0.swap(0, 2);
    }
    let (w, h) = img.dimensions();
    let buf = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, img.into_raw())?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(buf)])))
}

/// Кэш иконок (по `Arc<RenderImage>` на id). Один на окно настроек.
pub struct IconSet {
    /// Реальные id `{id}.png` из каталога, отсортированы. Id могут быть с дырками.
    pub ids: Vec<u32>,
    cache: HashMap<u32, Option<Arc<RenderImage>>>,
}

impl IconSet {
    pub fn discover() -> Self {
        let mut ids: Vec<u32> = std::fs::read_dir(icons_dir())
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let path = e.path();
                        (path.extension().is_some_and(|x| x == "png"))
                            .then(|| path.file_stem()?.to_string_lossy().parse::<u32>().ok())
                            .flatten()
                    })
                    .collect()
            })
            .unwrap_or_default();
        ids.sort_unstable();
        ids.dedup();
        Self {
            ids,
            cache: HashMap::new(),
        }
    }

    /// Иконка по id (лениво грузит + кэширует). None — если файла нет/битый.
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
