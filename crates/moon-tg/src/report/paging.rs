//! How many rows of a chat report go on one message.
//!
//! Every view (exchanges, cores, days) shows all its rows in one message while that message fits
//! Telegram's rich-message caps. Only an oversized report pages, and then with the largest page
//! from a fixed ladder that still fits, so a long roster does not turn into dozens of six-row
//! pages.
//!
//! The ladder, rather than the exact largest size, is for the inline Next/Prev buttons: they carry
//! only a page index, and the size is worked out again on every press. A trade closing between two
//! presses changes the content a little; an exact size would move with it and the same index would
//! then skip or repeat rows, while a ladder rung almost never changes.

#[cfg(test)]
mod tests;

/// Page sizes an oversized report may use, largest first.
const LADDER: [usize; 13] = [128, 96, 64, 48, 32, 24, 16, 12, 8, 6, 4, 2, 1];

/// The page size for `rows`: every row when the whole list fits, otherwise the largest ladder rung
/// whose every page fits.
///
/// Args:
///     rows: Every row of the report, in display order.
///     fits: Whether one page holding exactly these rows fits the message caps. It is called with
///         the whole list first, then with candidate pages.
///
/// Returns:
///     A page size of at least 1. When even single rows do not fit, 1 — the renderer then answers
///     with its own delivery-failure text rather than a cut table.
pub(super) fn fitting_page_size<T>(rows: &[T], fits: impl Fn(&[T]) -> bool) -> usize {
    let len = rows.len();
    if len == 0 || fits(rows) {
        return len.max(1);
    }
    LADDER
        .into_iter()
        .filter(|&size| size < len)
        .find(|&size| rows.chunks(size).all(&fits))
        .unwrap_or(1)
}
