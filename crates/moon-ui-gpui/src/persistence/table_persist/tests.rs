//! Regression tests for shared table-sort preference updates.

use std::collections::HashMap;

use moon_core::config::TableSortPreference;

use super::{update_core_status_mode, update_sort_preferences};

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

/// `table_persist.rs:update_core_status_mode` must change only the requested context and report
/// no-op repeats. Returning true for an identical code, or overwriting `:win` from `:dock`, would
/// schedule needless layout flushes or silently replace a detached panel's remembered tab.
#[test]
fn core_status_mode_updates_are_context_isolated_and_compare_then_mark() {
    let mut modes = HashMap::new();
    assert!(update_core_status_mode(
        &mut modes,
        "core-status-mode:win",
        "warnings",
    ));
    assert!(update_core_status_mode(
        &mut modes,
        "core-status-mode:dock",
        "flat",
    ));
    assert!(!update_core_status_mode(
        &mut modes,
        "core-status-mode:dock",
        "flat",
    ));
    assert!(update_core_status_mode(
        &mut modes,
        "core-status-mode:dock",
        "by-ip",
    ));

    assert_eq!(
        modes.get("core-status-mode:dock").map(String::as_str),
        Some("by-ip")
    );
    assert_eq!(
        modes.get("core-status-mode:win").map(String::as_str),
        Some("warnings")
    );
}
