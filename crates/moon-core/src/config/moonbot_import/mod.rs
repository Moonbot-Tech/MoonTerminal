//! Imports MoonBot settings from the clipboard (`MBSC7`) using a pure parser without GPUI
//! or side effects.
//!
//! Layers:
//! - [`transport`] finds `MBSC7:` in text and handles the hex header, Base16384, CRC32, gzip,
//!   and limits;
//! - [`reader`] is a bounded little-endian reader (`WriteStringX` strings, INI lists, bounds);
//! - [`schema_v7`] models the decompressed `MBSP` v7 payload (UI v3 / Theme v1 / Ini v1
//!   blocks; Signals/Trading/Visual presence is validated while their contents are skipped);
//! - [`shortcut`] decodes Delphi `TShortCut` values (modifiers + Windows VK).
//!
//! All reads use explicit little-endian primitives and checked arithmetic without `unsafe`.
//! Any corruption, truncation, or newer format rejects the entire import; no partial result
//! is returned.

pub mod apply;
pub mod plan;
pub mod reader;
pub mod schema_v7;
pub mod shortcut;
pub mod transport;

pub use apply::apply_local;
pub use plan::{MoonBotImportPlan, PlanContext};
pub use schema_v7::MoonBotConfig;

/// Import error. Messages use plain Russian text because the core is i18n-agnostic and the UI
/// displays them as-is. Each explains WHAT is wrong without raw payload dumps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    /// No supported `MBSC7:` header was found; older headers such as `MBSC6:` count as not found.
    NotFound,
    /// The format is newer than supported (`MBSC8+` or a payload version above 7).
    NewerFormat { found: u32 },
    /// Malformed transport/container header or invalid container magic.
    BadHeader(String),
    /// Base16384 error: out-of-range, extra, or missing character, or nonzero padding.
    BadBase16384(String),
    /// The compressed data's CRC32 does not match the declared value.
    CrcMismatch { expected: u32, actual: u32 },
    /// A hard size limit was exceeded (compressed/decompressed data, string, or list).
    TooLarge(String),
    /// gzip/zlib decompression error.
    Decompress(String),
    /// Data ended before the format expected it (`what` identifies the read operation).
    Truncated(String),
    /// Invalid payload field or block value, including an unsupported block version.
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

/// Fully parses clipboard text: transport (`MBSC7` → gzip bytes → payload) plus schema
/// (`MBSP` v7 → [`MoonBotConfig`]). Does not write or send anything.
pub fn parse_clipboard(text: &str) -> Result<MoonBotConfig, ImportError> {
    let payload = transport::decode_transport(text)?;
    schema_v7::parse_payload(&payload)
}
