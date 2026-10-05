//! Shared escaping and searchable names for Telegram HTML message text.
//!
//! Report replies and notification cards both insert names into a small tag
//! subset. Shared helpers keep escaping and hashtag spelling consistent across pushes.

/// One name cap for hashtags across cards, captions, outages and relayed core events.
pub(crate) const TAG_NAME_CHARS: usize = 64;

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

/// A name as one stable hashtag, or its original escaped text when it has no letter.
///
/// Args:
///     value: Coin or configured core name. Case and Unicode letters are preserved;
///         punctuation and whitespace become underscores before HTML escaping.
///     max_chars: Maximum Unicode scalars retained before mapping.
///     bold_fallback: Preserve bold untaggable coins on core-event pushes; false for
///         cores and for cards whose coin already sits inside bold tags.
///
/// Returns:
///     A hashtag with at least one letter, or the capped, escaped original name.
///     Untaggable names stay plain unless `bold_fallback` is true; empty stays empty.
pub(crate) fn name_tag(value: &str, max_chars: usize, bold_fallback: bool) -> String {
    let tag: String = value
        .chars()
        .take(max_chars)
        .map(|c| {
            // Rust's alphabetic/numeric properties include marks and non-decimal numbers
            // that Telegram cannot keep in a tag. Keep common decimal scripts explicitly.
            let combining = matches!(c, '\u{0300}'..='\u{036f}' | '\u{1ab0}'..='\u{1aff}'
                | '\u{1dc0}'..='\u{1dff}' | '\u{20d0}'..='\u{20ff}' | '\u{fe20}'..='\u{fe2f}');
            let decimal = c.is_ascii_digit()
                || matches!(c, '\u{0660}'..='\u{0669}' | '\u{06f0}'..='\u{06f9}' | '\u{ff10}'..='\u{ff19}');
            if c == '_' || (!combining && c.is_alphanumeric() && (!c.is_numeric() || decimal)) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if tag.chars().any(char::is_alphabetic) {
        format!("#{tag}")
    } else {
        let shown = escape(&value.chars().take(max_chars).collect::<String>());
        if bold_fallback && !shown.is_empty() {
            format!("<b>{shown}</b>")
        } else {
            shown
        }
    }
}

#[cfg(test)]
mod tests;
