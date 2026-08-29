//! Regression tests for restoring Screener header sorting.

use std::collections::HashSet;

use moon_core::config::TableSortPreference;

use super::{COLS, column_title, restore_sort};

/// The footer labels must resolve from the same schema as the Market and Vol. headers.
///
/// Mutation: restore independent `Coin` or `DVol` footer literals. The footer would again disagree
/// with the headers and the source assertion below would redden.
#[test]
fn screener_filter_labels_reuse_matching_column_titles() {
    assert_eq!(column_title("market"), "Market");
    assert_eq!(column_title("vol24"), "Vol.");

    let view_source = include_str!("../view.rs");
    assert!(view_source.contains("label(column_title(\"market\"))"));
    assert!(view_source.contains("label(column_title(\"vol24\"))"));
    assert!(!view_source.contains("label(\"Coin\")"));
    assert!(!view_source.contains("label(\"DVol\")"));
}

/// `screener/table.rs:restore_sort` must translate MoonUI ascending into the existing `desc` flag.
///
/// Mutation: copy `ascending` directly into `desc`. A saved ascending Core sort would reopen with a
/// descending row order while MoonUI draws an up arrow, and this assertion reddens.
#[test]
fn screener_restore_translates_ascending_to_descending_flag() {
    let visible = COLS.iter().map(|column| column.0.to_string()).collect();
    assert_eq!(
        restore_sort(
            Some(TableSortPreference {
                column: "core".to_string(),
                ascending: true,
            }),
            &visible
        ),
        ("core".to_string(), false)
    );
}

/// `screener/table.rs:restore_sort` must retain Vol.-descending for absent or retired preferences.
///
/// Mutation: accept an unknown key or default to ascending. Existing users would reopen on a
/// non-column or reversed familiar order, and one of these assertions reddens.
#[test]
fn screener_unknown_sort_keeps_the_historical_default() {
    let visible = COLS.iter().map(|column| column.0.to_string()).collect();
    assert_eq!(restore_sort(None, &visible), ("vol24".to_string(), true));
    assert_eq!(
        restore_sort(
            Some(TableSortPreference {
                column: "retired".to_string(),
                ascending: true,
            }),
            &visible
        ),
        ("vol24".to_string(), true)
    );
}

/// `screener/table.rs:restore_sort` must never restore an active key hidden from the user.
///
/// Mutation: omit the visibility check. A saved Core sort would remain active after Core was
/// hidden, leaving no visible header or arrow that could explain or reverse the order.
#[test]
fn screener_hidden_sort_uses_a_visible_fallback() {
    let visible = HashSet::from(["market".to_string(), "ask".to_string()]);
    assert_eq!(
        restore_sort(
            Some(TableSortPreference {
                column: "core".to_string(),
                ascending: true,
            }),
            &visible,
        ),
        ("market".to_string(), true)
    );
}
