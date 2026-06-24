//! Единый источник правды по палитре проекта — sRGB-байты из `:root` стенда
//! (stand-tauri/src/styles.css). Намеренно без зависимостей (egui/serde): на эти
//! константы ссылаются и `config` (дефолты `theme.toml` как `[u8; 3]`), и `shell`
//! (egui-хром как `Color32`), и цвет сервера по умолчанию. Менять цвет — здесь,
//! одно место; раньше один и тот же литерал был размазан по 4–5 файлам.
//!
//! Конвертация в linear для шейдеров — на стороне чарта (см. [[srgb-shader-colors]]).

/// --bg: фон панелей/тулбаров/чарта/стакана.
pub const BG: [u8; 3] = [0x13, 0x14, 0x16];
/// --surface-1: фон окна / закрытого контейнера.
pub const SURFACE_1: [u8; 3] = [0x1a, 0x1c, 0x1f];
/// Едва заметная сетка чарта.
pub const GRID: [u8; 3] = [0x17, 0x18, 0x1a];

/// --text: основной текст.
pub const TEXT: [u8; 3] = [0xe8, 0xe4, 0xdc];
/// --text-2: приглушённый текст (он же MUTED в хроме).
pub const TEXT_2: [u8; 3] = [0x97, 0x92, 0x8a];
/// --text-3: самый тусклый текст.
pub const TEXT_3: [u8; 3] = [0x5e, 0x5a, 0x53];
/// --hairline-strong: сильная рамка-hairline.
pub const HAIRLINE_STRONG: [u8; 3] = [0x3a, 0x3e, 0x45];

/// ≈ --lift: фон кнопок в покое.
pub const LIFT: [u8; 3] = [0x1d, 0x1f, 0x22];
/// ≈ --lift-hover: фон кнопок при наведении.
pub const LIFT_HOVER: [u8; 3] = [0x26, 0x28, 0x2d];
/// --lift-active: фон кнопок при нажатии.
pub const LIFT_ACTIVE: [u8; 3] = [0x2c, 0x2f, 0x35];

/// --accent: акцент (янтарный) — перекрестие, активные элементы, цвет сервера по умолчанию.
pub const ACCENT: [u8; 3] = [0xff, 0xb3, 0x47];
/// --long: зелёный (bid-сторона стакана / лонг).
pub const GREEN: [u8; 3] = [0x2f, 0xa8, 0x5c];
/// --sl: красный (stop-loss / шорт).
pub const RED: [u8; 3] = [0xff, 0x4a, 0x4a];
/// --short: оранжевый (ask-сторона стакана).
pub const ORANGE: [u8; 3] = [0xff, 0x8e, 0x5a];
/// --tp: голубой (take-profit).
pub const TP: [u8; 3] = [0x7f, 0xc9, 0xff];
