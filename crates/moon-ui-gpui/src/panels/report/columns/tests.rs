//! Report column formatting and persisted quote-identity regression tests.

// Explicit imports on purpose: the parent re-exports `gpui::*`, whose `test` would
// shadow the built-in attribute and make `#[test]` expand recursively.
use super::{
    basecurrency_text, cell_display_text, header_for, is_numeric_report_column, value_to_string,
};
use rusqlite::types::Value;

/// A generic text cell uses the shared flattener and trims its edges.
#[test]
fn drawn_cells_fold_breaks_and_trim_edges() {
    let raw = Value::Text(" a\r\nb ".to_string());
    assert_eq!(cell_display_text(&raw), "a ¶ b");
}

/// Text used as an identity remains verbatim.
#[test]
fn identity_values_are_not_reshaped() {
    let raw = Value::Text(" SPKUSDT ".to_string());
    assert_eq!(value_to_string(&raw), " SPKUSDT ");
}

/// Non-text values use the raw conversion formatting.
#[test]
fn non_text_values_are_untouched() {
    assert_eq!(cell_display_text(&Value::Null), "");
    assert_eq!(cell_display_text(&Value::Integer(42)), "42");
    assert_eq!(cell_display_text(&Value::Real(1.5)), "1.5");
}

/// Removing the synthetic Profit % header or numeric classification must fail these assertions;
/// otherwise the new column either exposes its internal key or aligns unlike its values.
#[test]
fn profit_percent_has_a_readable_numeric_column_contract() {
    assert_eq!(header_for("profitpct"), "profit %");
    assert!(is_numeric_report_column("profitpct"));
}

/// Persisted quote ordinals must display as exact tickers rather than opaque integers.
///
/// Removing `columns.rs:basecurrency_text` makes a USDC report row show `8`, hiding the identity
/// that makes its profit and totals meaningfully different from USDT.
#[test]
fn basecurrency_column_decodes_known_quote_ordinals() {
    assert_eq!(basecurrency_text(&Value::Integer(8)), "USDC");
    assert_eq!(basecurrency_text(&Value::Integer(0)), "BTC");
    assert_eq!(
        basecurrency_text(&Value::Integer(26)),
        "26",
        "unknown persisted ordinals must not be guessed"
    );
}
