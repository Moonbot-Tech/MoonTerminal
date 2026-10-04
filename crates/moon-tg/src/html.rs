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

/// A coin as a Telegram hashtag (`#MARSCOIN`), as the cores' own bot writes it, so a tap lists
/// every message about that coin.
///
/// A hashtag holds letters, digits and `_`; any other character becomes `_`. A coin with no
/// letter would not be recognised as a tag and shows bold instead; an empty coin is empty. The
/// result needs no escaping.
///
/// Args:
///     coin: The coin token, cut to `max_chars` Unicode scalars first.
///     max_chars: Longest coin kept.
pub(crate) fn coin_tag(coin: &str, max_chars: usize) -> String {
    let tag: String = coin
        .chars()
        .take(max_chars)
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    if tag.chars().any(char::is_alphabetic) {
        format!("#{tag}")
    } else if tag.is_empty() {
        String::new()
    } else {
        format!(
            "<b>{}</b>",
            escape(&coin.chars().take(max_chars).collect::<String>())
        )
    }
}

#[cfg(test)]
mod tests;
