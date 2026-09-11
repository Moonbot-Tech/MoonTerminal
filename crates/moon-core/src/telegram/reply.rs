//! Split a composed reply into Telegram-safe pages.
//!
//! Pages are counted in UTF-16 code units as the Bot API does. The UI adapter owns localized
//! string composition; this module never chooses user-facing prose.

/// Telegram `sendMessage` text limit, counted in UTF-16 code units as the Bot API does.
pub const TELEGRAM_MESSAGE_UTF16_LIMIT: usize = 4096;

/// Split already-composed text into pages of at most [`TELEGRAM_MESSAGE_UTF16_LIMIT`] UTF-16
/// units, on Unicode scalar (codepoint) boundaries, preferring newlines then spaces.
///
/// Args:
///     text: Localized payload produced by the UI adapter. Must not be chosen here.
///
/// Returns:
///     Ordered pages. An empty input yields one empty page so a send still happens.
pub fn segment_pages(text: &str) -> Vec<String> {
    if utf16_len(text) <= TELEGRAM_MESSAGE_UTF16_LIMIT {
        return vec![text.to_string()];
    }
    let mut pages = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let (page, tail) = split_one_page(rest);
        pages.push(page.to_string());
        rest = tail;
    }
    if pages.is_empty() {
        pages.push(String::new());
    }
    pages
}

/// UTF-16 code unit count, matching Telegram's 4096-character budget.
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Split one Telegram-sized page, preferring a newline or whitespace boundary.
fn split_one_page(text: &str) -> (&str, &str) {
    if utf16_len(text) <= TELEGRAM_MESSAGE_UTF16_LIMIT {
        return (text, "");
    }
    let mut units = 0usize;
    let mut last_char = 0usize;
    let mut last_space = None;
    let mut last_newline = None;
    for (idx, ch) in text.char_indices() {
        let ch_units = ch.len_utf16();
        if units + ch_units > TELEGRAM_MESSAGE_UTF16_LIMIT {
            let at = last_newline.or(last_space).unwrap_or(last_char);
            if at == 0 {
                // Single scalar wider than the limit cannot occur for BMP/SMP chars (max 2
                // units). Fall back to the last complete scalar so the loop always shrinks.
                let at = if last_char == 0 { idx } else { last_char };
                return split_at_trim(text, at);
            }
            return split_at_trim(text, at);
        }
        units += ch_units;
        let end = idx + ch.len_utf8();
        last_char = end;
        if ch == '\n' {
            last_newline = Some(end);
        } else if ch.is_whitespace() {
            last_space = Some(end);
        }
    }
    (text, "")
}

/// Trim the page boundary without splitting a UTF-8 scalar or duplicating whitespace.
fn split_at_trim(text: &str, at: usize) -> (&str, &str) {
    let at = at.min(text.len());
    let (head, tail) = text.split_at(at);
    (head.trim_end_matches(['\n', '\r']), tail.trim_start())
}
