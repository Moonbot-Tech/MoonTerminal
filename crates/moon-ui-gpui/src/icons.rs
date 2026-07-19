//! Иконки групп из `assets/icons/{id}.png` — порт egui `src/icons.rs` на gpui.
//! Грузит PNG (image crate) в `RenderImage` (BGRA, как ждёт gpui от `img(..)`),
//! кэширует по id. Набор ВШИТ в бинарь (`include_dir`) — едет со сборкой сам; файл на диске
//! (рядом с cwd/exe) имеет приоритет, чтобы докидывать/подменять иконки без пересборки.
//! (Как `coin_icons.rs`. До этого читали только с диска — у пользователей без папки
//! `assets/icons` пикер был пустой; см. п.2 UX-фидбека.)

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::RenderImage;
use image::{Frame, ImageBuffer, Rgba};
use include_dir::{Dir, include_dir};

/// Вшитый набор иконок групп (весь `assets/icons`, ~64 КБ PNG).
static EMBEDDED: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/icons");

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

/// `{stem}.png` → id, если stem — число. Общий парсер для диска и вшитого набора.
fn id_from_png_name(name: &str) -> Option<u32> {
    name.strip_suffix(".png")?.parse::<u32>().ok()
}

/// Загрузить `{id}.png` в `RenderImage` (BGRA — gpui свопает R/B, как и для чарта).
/// Диск (приоритет — можно докидывать иконки) → вшитый набор.
fn load_render_image(id: u32) -> Option<Arc<RenderImage>> {
    let file = format!("{id}.png");
    let bytes: Vec<u8> = match std::fs::read(icons_dir().join(&file)) {
        Ok(b) => b,
        Err(_) => EMBEDDED.get_file(&file)?.contents().to_vec(),
    };
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
        // Вшитый набор — базовый (всегда есть); диск — дополнение/подмена (можно докидывать
        // свои иконки без пересборки). Объединяем id обоих источников.
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
