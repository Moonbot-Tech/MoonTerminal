//! Shared escape for Telegram HTML message text.
//!
//! Report replies and notification cards both insert names into a small tag
//! subset. One helper keeps those surfaces on the same four replacements.

/// Escape `&`, `<`, `>`, and `"` for Telegram HTML.
///
/// Truncate on a char boundary before calling when a field has a length cap.
/// Escaping first would let a cut land inside an entity and split a tag.
///
/// Args:
///     value: Raw text. Apostrophes are left alone; Telegram HTML does not
///         treat them as markup.
///
/// Returns:
///     Text safe to place between tags.
pub(crate) fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
