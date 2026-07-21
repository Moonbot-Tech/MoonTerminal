//! Normalizes arbitrary feed and database text for single-line UI surfaces.
//!
//! GPUI lays out one visual line per hard `\n`, including in fixed-height rows that disable
//! soft wrapping — `whitespace_nowrap` and `truncate` suppress only SOFT wrapping. Such a
//! row centres the stacked lines and clips them, so the neighbours show up as half-cut
//! glyphs that read as an extra, broken row; that is the symptom this module exists for.
//! [`flatten_lines`] folds hard breaks for report cells, log rows, and single-line
//! diagnostics. Report exports use raw database values.

/// Visible marker inserted for a folded line break.
///
/// U+00B6 is available in both bundled fonts; U+23CE would require a system fallback font.
/// The marker is not localized because it contains no translatable text.
const BREAK_MARK: &str = " ¶ ";

/// Folds embedded CRLF, lone CR, and lone LF separators into [`BREAK_MARK`].
///
/// Leading and trailing line-break characters are removed. The returned string contains no
/// raw line breaks, preserves all other whitespace, and is unchanged by another call.
pub(crate) fn flatten_lines(text: &str) -> String {
    let text = text.trim_matches(['\r', '\n']);
    // Avoid the two replacement allocations when the text is already a single line.
    if !text.contains(['\n', '\r']) {
        return text.to_string();
    }
    // Normalize CRLF first so each pair produces one marker.
    text.replace("\r\n", "\n").replace(['\r', '\n'], BREAK_MARK)
}

#[cfg(test)]
mod tests;
