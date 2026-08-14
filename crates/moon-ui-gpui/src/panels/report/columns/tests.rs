//! Report column formatting and persisted quote-identity regression tests.

// Explicit imports on purpose: the parent re-exports `gpui::*`, whose `test` would
// shadow the built-in attribute and make `#[test]` expand recursively.
use super::{
    basecurrency_text, cell_display_text, cell_tooltip, effective_visible_columns, header_for,
    is_numeric_report_column, toggled_all_columns, value_to_string,
};
use rusqlite::types::Value;
use std::collections::HashSet;

/// Breakage: changing `cell_tooltip` to return `None` for the valuation-source column hides the
/// second conversion leg and successor delay even though the database retained both.
#[test]
fn valuation_source_tooltip_keeps_complete_provenance() {
    let provenance = "hyperliquid_spot USDH/USDC -> binance_spot USDCUSDT +126720m";
    assert_eq!(
        cell_tooltip(moon_core::db::VALUATION_SOURCE_COLUMN, provenance).as_deref(),
        Some(provenance)
    );
    assert_eq!(cell_tooltip("coin", provenance), None);
    assert_eq!(
        cell_tooltip(moon_core::db::VALUATION_SOURCE_COLUMN, ""),
        None
    );
}

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

/// `columns.rs:effective_visible_columns` must remove only `core_name` when its AutoCore
/// flag is true. Filtering by saved-set cardinality instead would also reveal the user-hidden
/// `comment` column or hide `core_name` in Overview and Classic.
#[test]
fn auto_core_lens_removes_only_core_name_from_the_saved_visible_set() {
    let cols = ["closedate", "core_name", "coin", "comment"]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let visible = HashSet::from(["core_name".to_string(), "coin".to_string()]);

    assert_eq!(
        effective_visible_columns(&cols, &visible, false)
            .map(|(index, _)| index)
            .collect::<Vec<_>>(),
        vec![1, 2],
        "Classic, Overview, and standalone use the raw saved preference"
    );
    assert_eq!(
        effective_visible_columns(&cols, &visible, true)
            .map(|(index, _)| index)
            .collect::<Vec<_>>(),
        vec![2],
        "AutoCore hides core_name but must not reveal another saved-hidden column"
    );
}

/// `columns.rs:toggled_all_columns` must modify only columns available in the current context.
/// Rebuilding the set from available names would erase dormant `core_name`, so returning to
/// Overview would lose the user's prior preference.
#[test]
fn auto_core_all_toggle_preserves_the_dormant_core_name_preference() {
    let cols = ["closedate", "core_name", "coin"]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let partial = HashSet::from(["core_name".to_string(), "coin".to_string()]);

    let all_on = toggled_all_columns(&cols, &partial, true);
    assert_eq!(
        all_on,
        HashSet::from([
            "closedate".to_string(),
            "core_name".to_string(),
            "coin".to_string(),
        ])
    );
    assert_eq!(
        toggled_all_columns(&cols, &all_on, true),
        HashSet::from(["closedate".to_string(), "core_name".to_string()]),
        "turning available columns off keeps the first available and dormant core_name"
    );

    let core_name_hidden = HashSet::from(["coin".to_string()]);
    assert_eq!(
        toggled_all_columns(&cols, &core_name_hidden, true),
        HashSet::from(["closedate".to_string(), "coin".to_string()]),
        "All must not re-enable a dormant preference that the user saved hidden"
    );
}
