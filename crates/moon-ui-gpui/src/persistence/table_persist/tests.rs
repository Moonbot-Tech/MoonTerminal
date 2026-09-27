//! Regression tests for shared table-sort preference updates.

use std::collections::HashMap;

use gpui::SharedString;
use moon_core::config::TableSortPreference;

use super::{
    merge_column_order, remember_column_order, update_column_order, update_core_status_mode,
    update_sort_preferences,
};

/// Context ids of the tables whose dragged order this module stores.
///
/// Report, Screener, Assets, and each Core Status data table, dock and window where both exist.
/// Screener is window-only. By IP is absent: that view is a tree with a fixed column sequence.
const COLUMN_ORDER_IDS: &[&str] = &[
    "report-table-v2:dock",
    "report-table-v2:win",
    "screener-table:win",
    "assets-table:dock",
    "assets-table:win",
    "core-status-table:dock",
    "core-status-table:win",
    "core-status-problems:dock",
    "core-status-problems:win",
    "core-status-warnings:dock",
    "core-status-warnings:win",
    "core-status-updates:dock",
    "core-status-updates:win",
];

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

/// `table_persist.rs:update_column_order` must round-trip each listed table without crossing contexts.
///
/// Mutation: share one list across every id, or return true when the stored list is repeated. A
/// docked Report drag would then replace the detached window's order, or every unchanged resize
/// would arm a layout flush.
#[test]
fn column_order_round_trips_per_listed_table_and_ignores_repeats() {
    let mut orders = HashMap::new();
    for (index, id) in COLUMN_ORDER_IDS.iter().enumerate() {
        let order = vec![format!("col-{index}-b"), format!("col-{index}-a")];
        assert!(update_column_order(&mut orders, id, &order));
        assert!(!update_column_order(&mut orders, id, &order));
        assert_eq!(orders.get(*id), Some(&order));
    }
    assert_eq!(orders.len(), COLUMN_ORDER_IDS.len());

    assert!(update_column_order(
        &mut orders,
        "report-table-v2:dock",
        &[],
    ));
    assert!(!orders.contains_key("report-table-v2:dock"));
    assert!(!update_column_order(
        &mut orders,
        "report-table-v2:dock",
        &[],
    ));
    assert_eq!(
        orders.get("report-table-v2:win").map(Vec::as_slice),
        Some(["col-1-b".to_string(), "col-1-a".to_string()].as_slice())
    );
}

/// `table_persist.rs:merge_column_order` must drop a removed id and append a new one.
///
/// Mutation: keep an unknown id, or insert a new id at its source index instead of the tail. A
/// renamed column would then stay in the saved sequence, or a column added later would jump into
/// the middle of an order the user arranged. An empty saved list must stay empty so a table that
/// was never dragged still follows its source order.
#[test]
fn column_order_merge_drops_unknown_ids_and_appends_new_ones() {
    let stored = ["pnl", "gone", "pnl", "coin", ""]
        .map(str::to_string)
        .to_vec();
    let live = ["coin", "qty", "pnl"];
    assert_eq!(
        merge_column_order(&stored, &live),
        vec!["pnl".to_string(), "coin".to_string(), "qty".to_string()]
    );

    assert!(merge_column_order(&[], &live).is_empty());
    assert_eq!(
        merge_column_order(&stored, &[]),
        stored,
        "an unloaded schema must not discard the saved order"
    );

    let obsolete = ["retired".to_string()];
    assert_eq!(
        merge_column_order(&obsolete, &live),
        vec!["coin".to_string(), "qty".to_string(), "pnl".to_string()]
    );
}

/// `table_persist.rs:remember_column_order` must ignore a notification that did not move columns.
///
/// Mutation: return `true` from the equal-slice branch. A second open table that shares the
/// context id would then write its stale order on a resize or a header click, and the `false`
/// assertion reddens.
#[test]
fn unchanged_local_column_order_is_not_a_write() {
    let mut seen = vec![SharedString::from("pnl"), SharedString::from("coin")];
    let same = seen.clone();
    assert!(!remember_column_order(&mut seen, &same));
    assert_eq!(seen, same);

    let dragged = vec![SharedString::from("coin"), SharedString::from("pnl")];
    assert!(remember_column_order(&mut seen, &dragged));
    assert_eq!(seen, dragged);
    assert!(!remember_column_order(&mut seen, &dragged));
}
