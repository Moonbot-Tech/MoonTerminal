//! Paste/Create target precedence plus UI-only folder occupancy and observer wiring.

use moon_core::feed::StrategyRow;
use moon_core::session::CoreId;

use super::{footer_labels_fit, keep_ui_folder, resolve_paste_target};

/// The source file, read at COMPILE time so the guard below cannot drift from it.
const SRC: &str = include_str!("../ui.rs");
/// The observer must retire UI-only folders when the live strategy snapshot changes.
const STATE_SRC: &str = include_str!("../../state.rs");

/// Build the only strategy fields the folder-occupancy decision reads.
fn strategy(id: u64, folder_path: &str) -> StrategyRow {
    StrategyRow {
        id,
        name: format!("strategy-{id}"),
        kind: "Demo".to_string(),
        kind_ordinal: 1,
        folder_path: folder_path.to_string(),
        checked: false,
        is_short: false,
        fields: Vec::new(),
    }
}

/// Build a selected folder or strategy location for precedence tests.
fn at(core: CoreId, path: &str) -> Option<(CoreId, String)> {
    Some((core, path.to_string()))
}

/// `default_target` must actually DELEGATE, or the precedence proven below protects nothing.
///
/// Plausible edit this catches: inlining a strategy-first
/// `if let Some((core, id)) = self.selected` body in `default_target` would bypass the shared
/// precedence while every pure-function assertion in this file stayed green.
#[test]
fn default_target_resolves_through_the_shared_precedence() {
    let start = SRC
        .find("fn default_target")
        .expect("default_target must exist");
    let body = &SRC[start..];
    let end = body.find("\n    }").expect("its body must end");
    let body = &body[..end];
    assert!(
        body.contains("resolve_paste_target("),
        "default_target must delegate to resolve_paste_target"
    );
    assert!(
        !body.contains("return (core,"),
        "default_target must not re-implement its own precedence"
    );
}

/// A clicked folder wins over whatever strategy happens to still be the primary selection.
///
/// Plausible edit this catches: the two arms are reordered, or the folder arm is dropped
/// because "`selected` is always set anyway" — and Ctrl+V after clicking a folder lands in an
/// unrelated strategy's folder, or in the core root, building a second folder tree beside the
/// one the user was looking at.
#[test]
fn a_selected_folder_outranks_a_stale_strategy() {
    assert_eq!(
        resolve_paste_target(at(7, "grid/live"), at(3, "old"), Some(1)),
        (7, "grid/live".to_string()),
        "the folder was the last thing clicked, so it is the target — core included"
    );
}

/// With no folder selected, the primary strategy's own folder is the target.
#[test]
fn without_a_folder_the_strategy_supplies_the_target() {
    assert_eq!(
        resolve_paste_target(None, at(3, "old"), Some(1)),
        (3, "old".to_string())
    );
}

/// Nothing selected at all falls back to the first core's root — and to core 0 when there is
/// not even a core, rather than panicking on an empty tree.
#[test]
fn nothing_selected_falls_back_to_the_first_cores_root() {
    assert_eq!(
        resolve_paste_target(None, None, Some(1)),
        (1, String::new())
    );
    assert_eq!(resolve_paste_target(None, None, None), (0, String::new()));
}

/// A folder selected at the core ROOT is still a real answer, not an absent one.
#[test]
fn a_root_folder_selection_is_not_mistaken_for_nothing() {
    assert_eq!(
        resolve_paste_target(at(9, ""), at(3, "old"), Some(1)),
        (9, String::new()),
        "an empty path on core 9 means that core's root, not 'no selection'"
    );
}

/// `strategies/tree/ui.rs:keep_ui_folder`: replacing subtree occupancy with direct equality would
/// preserve a parent marker and reveal a ghost folder after the child strategy is later deleted.
#[test]
fn a_live_strategy_occupies_its_folder_and_every_ui_only_ancestor() {
    let rows = vec![strategy(1, "desk/live")];

    assert!(!keep_ui_folder("desk", Some(&rows)));
    assert!(!keep_ui_folder("desk/live", Some(&rows)));
    assert!(keep_ui_folder("desk/archive", Some(&rows)));
    assert!(keep_ui_folder("des", Some(&rows)));
    assert!(keep_ui_folder("desk", Some(&[])));
    assert!(keep_ui_folder("desk", None));
}

/// `strategies/state.rs:StrategiesView::observe_state`: removing the reconciliation call would
/// leave occupied markers cached and expose a ghost folder after an Analytics purge.
#[test]
fn strategy_snapshot_changes_reconcile_ui_only_folders() {
    let state_code: String = STATE_SRC
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        state_code.contains("this.reconcile_ui_folders(b.session.store());"),
        "the live snapshot observer must retire newly occupied UI-only folders"
    );
}

/// `tree/ui.rs:footer_labels_fit`: changing the inclusive threshold would either hide all labels
/// at the exact fitting width or show a mixed/clipped footer one pixel below its full requirement.
#[test]
fn footer_labels_switch_atomically_at_the_inclusive_boundary() {
    let fixed_chrome = 40.0;
    let buttons_and_staged_label = 60.0;

    assert!(!footer_labels_fit(
        99.0,
        fixed_chrome,
        buttons_and_staged_label
    ));
    assert!(footer_labels_fit(
        100.0,
        fixed_chrome,
        buttons_and_staged_label
    ));
    assert!(footer_labels_fit(
        101.0,
        fixed_chrome,
        buttons_and_staged_label
    ));
}
