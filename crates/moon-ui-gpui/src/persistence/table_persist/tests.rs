//! Regression tests for shared table-sort preference updates.

use std::collections::HashMap;

use moon_core::config::TableSortPreference;

use super::update_sort_preferences;

/// `table_persist.rs:update_sort_preferences` must preserve a descending choice verbatim.
///
/// Mutation: replace the incoming `ascending` value with `true` before insertion. The assertion on
/// `ascending` reddens, proving a descending header choice would otherwise restart as ascending.
#[test]
fn descending_sort_preference_survives_shared_update() {
    let mut preferences = HashMap::new();
    assert!(update_sort_preferences(
        &mut preferences,
        "orders-table:dock",
        Some(TableSortPreference {
            column: "pnl".to_string(),
            ascending: false,
        }),
    ));

    assert_eq!(
        preferences.get("orders-table:dock"),
        Some(&TableSortPreference {
            column: "pnl".to_string(),
            ascending: false,
        })
    );
}

/// `table_persist.rs:update_sort_preferences` must dirty only real insert/update/remove changes.
///
/// Mutation: return `true` from the equal-value branch. Repeating an unchanged header choice would
/// then arm the layout writer on every redundant callback, and the second assertion reddens.
#[test]
fn unchanged_sort_is_a_noop_and_default_removes_the_entry() {
    let mut preferences = HashMap::new();
    let saved = TableSortPreference {
        column: "coin".to_string(),
        ascending: true,
    };
    assert!(update_sort_preferences(
        &mut preferences,
        "assets-table:win",
        Some(saved.clone()),
    ));
    assert!(!update_sort_preferences(
        &mut preferences,
        "assets-table:win",
        Some(saved),
    ));
    assert!(update_sort_preferences(
        &mut preferences,
        "assets-table:win",
        None,
    ));
    assert!(!update_sort_preferences(
        &mut preferences,
        "assets-table:win",
        None,
    ));
    assert!(preferences.is_empty());
}
