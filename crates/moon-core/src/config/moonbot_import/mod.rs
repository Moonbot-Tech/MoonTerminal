//! Импорт настроек MoonBot из буфера обмена (`MBSC7`) — чистый parser без GPUI и без
//! побочных эффектов. ТЗ: `docs-internal/MOONBOT_CONFIG_IMPORT.md`.
//!
//! Слои:
//! - [`transport`] — поиск `MBSC7:` в тексте, hex-заголовок, Base16384, CRC32, gzip, лимиты;
//! - [`reader`] — bounded little-endian reader (строки `WriteStringX`, INI-списки, границы);
//! - [`schema_v7`] — модель распакованного payload `MBSP` v7 (блоки UI v3 / Theme v1 / Ini v1;
//!   Signals/Trading/Visual присутствие проверяем, содержимое пропускаем);
//! - [`shortcut`] — декодер Delphi `TShortCut` (модификаторы + Windows VK).
//!
//! Всё чтение — по явным little-endian примитивам, checked math, без `unsafe`. Любая
//! порча/усечение/слишком новый формат — ошибка целиком, частичный результат не отдаём.

pub mod apply;
pub mod plan;
pub mod reader;
pub mod schema_v7;
pub mod shortcut;
pub mod transport;

pub use apply::apply_local;
pub use plan::{MoonBotImportPlan, PlanContext};
pub use schema_v7::MoonBotConfig;

/// Ошибка импорта. Сообщения — простым русским текстом (core i18n-агностичен, UI
/// показывает как есть); каждая описывает, ЧТО именно не так, без сырых дампов payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    /// В тексте нет `MBSC7:` (и вообще заголовка MBSC).
    NotFound,
    /// Формат новее поддерживаемого (`MBSC8+` или payload-версия выше 7).
    NewerFormat { found: u32 },
    /// Синтаксис заголовка/контейнера (hex, magic, версия блока и т.п.).
    BadHeader(String),
    /// Base16384: символ вне диапазона, лишний/недостающий символ, ненулевой padding.
    BadBase16384(String),
    /// CRC32 сжатых данных не совпал с заявленным.
    CrcMismatch { expected: u32, actual: u32 },
    /// Превышен жёсткий лимит размера (сжатого/распакованного/строки/списка).
    TooLarge(String),
    /// Ошибка gzip/zlib-распаковки.
    Decompress(String),
    /// Данные закончились раньше, чем ожидает формат (`what` — что читали).
    Truncated(String),
    /// Недопустимое значение поля (`what` — какое и почему).
    BadValue(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "В тексте не найден блок настроек MoonBot (MBSC7)"),
            Self::NewerFormat { found } => write!(
                f,
                "Формат MBSC{found} новее этой версии MoonTerminal; обновите Terminal"
            ),
            Self::BadHeader(s) => write!(f, "Повреждённый заголовок: {s}"),
            Self::BadBase16384(s) => write!(f, "Повреждённая кодировка Base16384: {s}"),
            Self::CrcMismatch { expected, actual } => write!(
                f,
                "Контрольная сумма не совпала (ожидалась {expected:08X}, получена {actual:08X})"
            ),
            Self::TooLarge(s) => write!(f, "Превышен лимит размера: {s}"),
            Self::Decompress(s) => write!(f, "Ошибка распаковки: {s}"),
            Self::Truncated(s) => write!(f, "Данные обрываются: {s}"),
            Self::BadValue(s) => write!(f, "Недопустимое значение: {s}"),
        }
    }
}

impl std::error::Error for ImportError {}

/// Полный разбор текста из буфера обмена: транспорт (`MBSC7` → gzip-байты → payload)
/// плюс схема (`MBSP` v7 → [`MoonBotConfig`]). Ничего не пишет и не отправляет.
pub fn parse_clipboard(text: &str) -> Result<MoonBotConfig, ImportError> {
    let payload = transport::decode_transport(text)?;
    schema_v7::parse_payload(&payload)
}
