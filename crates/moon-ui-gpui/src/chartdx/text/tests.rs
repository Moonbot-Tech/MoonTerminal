//! Unit tests for the price a cursor's percentage and color are measured against.
//!
//! Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
//! would shadow the built-in `#[test]` attribute and make it expand recursively (CONTRIBUTING.md).
use crate::chartdx::text::{cursor_ref_price, fmt_prospective_order_size};

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

/// The chart-specific order-size helper must preserve meaningful fractions without fixed zeros.
///
/// Replacing the shared formatter with fixed hundredths makes these literal display contracts red.
#[test]
fn prospective_order_size_label_uses_shared_compact_formatter() {
    assert_eq!(fmt_prospective_order_size(1_500.0), "1.5k");
    assert_eq!(fmt_prospective_order_size(10_000.0), "10k");
    assert_eq!(fmt_prospective_order_size(1_000_000.0), "1m");
    assert_eq!(fmt_prospective_order_size(50.0), "50");
}

/// The retained chart render path must call the chart-specific compact order-size helper.
///
/// Restoring the former fixed-hundredths expression fails this source-wiring oracle even when the
/// helper's direct tests remain green.
#[test]
fn prepare_wires_compact_order_size_label() {
    let source = include_str!("prepare.rs");

    assert!(source.contains("let text = fmt_prospective_order_size(usd);"));
    assert!(!source.contains("format!(\"{usd:.2}\")"));
}

/// The chart time-step preparation must use the width-derived target rather than a fixed count.
///
/// Breakage this pins: restoring the former literal `6.0` target would under-label wide plots and
/// crowd narrow plots even though the pure axis helper's direct tests remain green.
#[test]
fn prepare_wires_width_derived_time_label_target() {
    let source = include_str!("prepare.rs");
    let compact: String = source.split_whitespace().collect();
    let call = "moon_chart::axes::nice_time_step(window_ms/1000.0,";

    assert!(
        compact.contains(&format!(
            "{call}moon_chart::axes::time_label_target(plot_w),)"
        )),
        "prepared time steps must receive the plot-width target"
    );
    assert!(
        !compact.contains(&format!("{call}6.0)")),
        "prepared time steps must not restore the fixed six-label target"
    );
}
