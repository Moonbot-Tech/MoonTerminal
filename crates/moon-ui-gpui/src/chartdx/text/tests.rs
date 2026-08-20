//! Unit tests for the price a cursor's percentage and color are measured against.
//!
//! Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
//! would shadow the built-in `#[test]` attribute and make it expand recursively (CONTRIBUTING.md).
use crate::chartdx::text::cursor_ref_price;

const LAST: f32 = 100.0;
const BOOK: Option<(f32, f32)> = Some((99.5, 100.5));

#[test]
fn above_the_last_price_measures_from_the_best_ask() {
    assert_eq!(cursor_ref_price(BOOK, LAST, 110.0), 100.5);
}

#[test]
fn below_the_last_price_measures_from_the_best_bid() {
    assert_eq!(cursor_ref_price(BOOK, LAST, 90.0), 99.5);
}

#[test]
fn the_last_price_itself_counts_as_above() {
    // The boundary must land on one side deterministically; matching the cursor block's own
    // `price >= last`, it takes the ask.
    assert_eq!(cursor_ref_price(BOOK, LAST, LAST), 100.5);
}

#[test]
fn without_a_book_the_reference_is_the_last_price() {
    // No book for the market yet, or the order book switched off for the window: `market.rs`
    // clears `book_best` in both cases rather than leaving a frozen bid/ask behind.
    assert_eq!(cursor_ref_price(None, LAST, 110.0), LAST);
    assert_eq!(cursor_ref_price(None, LAST, 90.0), LAST);
}

#[test]
fn a_one_sided_book_reads_the_same_from_either_direction() {
    // `best_bid_ask` reports the single populated side in both positions, so both directions
    // measure from it rather than one of them silently reverting to the last price.
    let one_sided = Some((100.5, 100.5));
    assert_eq!(cursor_ref_price(one_sided, LAST, 110.0), 100.5);
    assert_eq!(cursor_ref_price(one_sided, LAST, 90.0), 100.5);
}
