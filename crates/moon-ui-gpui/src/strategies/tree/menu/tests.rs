//! Regression tests for reorder menu entries and empty-folder refusal.

use super::reorder_entries;
use crate::strategies::tree::ops::{MoveStep, reorder_folder_step};
use moon_core::feed::StrategyRow;

/// Omitting the folder actions or enabling an empty folder would make the menu disagree with arrows.
#[test]
fn folder_menu_keeps_both_directions_and_explains_empty_refusal() {
    let rows: Vec<_> = ["A", "B", "C"]
        .into_iter()
        .enumerate()
        .map(|(id, path)| StrategyRow {
            id: id as u64,
            name: "Example".into(),
            kind: "Long".into(),
            kind_ordinal: 1,
            folder_path: path.into(),
            checked: false,
            is_short: false,
            fields: Vec::new(),
        })
        .collect();
    let refs: Vec<_> = rows.iter().collect();
    let ask = |folder: &str, step| reorder_folder_step(&refs, &[folder.into()], step).is_some();
    let entries = reorder_entries(ask("B", MoveStep::Up), ask("B", MoveStep::Down), false);
    assert_eq!(entries.each_ref().map(|e| e.key), ["move-up", "move-down"]);
    assert!(entries.iter().all(|e| e.enabled));
    let empty = reorder_entries(
        ask("Empty", MoveStep::Up),
        ask("Empty", MoveStep::Down),
        true,
    );
    assert!(empty.iter().all(|e| !e.enabled));
    assert!(
        empty
            .iter()
            .all(|e| e.right_label == rust_i18n::t!("strat.empty_folder_reorder"))
    );
}
