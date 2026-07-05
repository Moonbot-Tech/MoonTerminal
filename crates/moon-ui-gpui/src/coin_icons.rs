//! Значки монет из `assets/coins/{symbol}.png` (32×32 color, набор
//! spothq/cryptocurrency-icons, лицензия CC0 — см. `assets/coins/README.md`).
//! Набор ВШИТ в бинарь (`include_dir`) — иконки едут в сборку сами; файл на диске
//! (рядом с cwd/exe) имеет приоритет, чтобы докидывать новые монеты без пересборки.
//! Глобальный ленивый кэш по символу: PNG → `RenderImage` (BGRA, как ждёт gpui)
//! один раз; отсутствующие тоже кэшируются (None) — диск не перечитываем.
//! Нет значка → вызывающий рисует монету без иконки.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use gpui::RenderImage;
use image::{Frame, ImageBuffer, Rgba};
use include_dir::{include_dir, Dir};

/// Вшитый набор значков (весь `assets/coins`, ~412 КБ PNG).
static EMBEDDED: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/coins");

/// Каталог значков: `assets/coins` рядом с cwd, иначе рядом с exe (как `icons.rs`).
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

/// Ключ файла: нижний регистр; фьючерсные множители `1000PEPE`/`10000SATS` сводим к
/// базовому символу (иконка одна на монету).
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

/// Загрузить `{key}.png` в `RenderImage` (RGBA→BGRA — gpui свопает R/B).
/// Диск (приоритет — можно докидывать значки) → вшитый набор.
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

/// Значок монеты по символу (`"BTC"`/`"usdt"`/`"1000PEPE"`). None — значка нет.
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
