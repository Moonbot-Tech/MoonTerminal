// Explicit imports on purpose: the parent re-exports `gpui::*`, whose `test` would
// shadow the built-in attribute and make `#[test]` expand recursively.
use super::{cell_display_text, value_to_string};
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
