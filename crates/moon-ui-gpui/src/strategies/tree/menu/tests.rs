//! Regression tests for reorder menu entries and empty-folder refusal.

use super::{empty_folder_delete_entry, folder_delete_entry, reorder_entries};
use crate::strategies::tree::ops::{
    DeleteBlock, MoveStep, empty_folder_delete_plan, reorder_folder_step,
};
use moon_core::feed::StrategyRow;

/// A right-click inside a multi-folder selection must not bypass strict bulk deletion and delete the
/// clicked folder's disabled strategies through the one-folder path.
#[test]
fn folder_delete_menu_uses_the_current_selection_route() {
    let source = include_str!("../menu.rs");
    let folder = source
        .split_once("MenuTarget::Folder(path) =>")
        .expect("folder menu must exist")
        .1
        .split_once("MenuTarget::Strategy(id) =>")
        .expect("folder menu end must be found")
        .0;
    assert!(folder.contains("this.request_delete_target(window, cx)"));
    assert!(!folder.contains("this.request_delete_folder("));
}

/// Dropping the count or enabling a zero-target action would promise cleanup that cannot run.
#[test]
fn all_cores_empty_folder_menu_counts_targets_and_disables_at_zero() {
    let _locale = crate::test_locale::force("en");
    let some = empty_folder_delete_entry("strat.menu_delete_empty_all_cores", 2);
    assert_eq!(some.label, "Delete empty folders in every visible core (2)");
    assert!(!some.disabled);

    let none = empty_folder_delete_entry("strat.menu_delete_empty_all_cores", 0);
    assert_eq!(none.label, "Delete empty folders in every visible core (0)");
    assert!(none.disabled);
}

/// Counting the clicked folder's enabled strategies as a block would disable valid bulk cleanup.
#[test]
fn multi_folder_delete_counts_only_empty_targets_and_ignores_clicked_block() {
    let _locale = crate::test_locale::force("en");
    let rows = [StrategyRow {
        id: 1,
        name: "Example".into(),
        kind: "Long".into(),
        kind_ordinal: 1,
        folder_path: "occupied".into(),
        checked: true,
        is_short: false,
        fields: Vec::new(),
    }];
    let selected = [(7, "empty".to_string()), (7, "occupied".to_string())];
    let plan = empty_folder_delete_plan(&[(7, rows.as_slice())], &selected);
    assert_eq!(plan.skipped_non_empty, 1);

    let entry = folder_delete_entry(
        selected.len(),
        plan.targets.len(),
        Some(DeleteBlock {
            enabled: 1,
            total: 1,
        }),
    );
    assert_eq!(entry.label, "Delete empty folders (1)");
    assert!(!entry.disabled);
    assert!(entry.block.is_none());
    assert!(folder_delete_entry(2, 0, None).disabled);
}

/// The multi-selection model must not weaken the existing one-folder enabled-strategy guard.
#[test]
fn single_folder_delete_keeps_the_clicked_folder_block() {
    let _locale = crate::test_locale::force("en");
    let block = DeleteBlock {
        enabled: 1,
        total: 1,
    };
    let entry = folder_delete_entry(1, 0, Some(block));
    assert_eq!(entry.label, "Delete folder…");
    assert!(entry.disabled);
    assert_eq!(entry.block, Some(block));
}

/// Omitting the folder actions or enabling an empty folder would make the menu disagree with arrows.
#[test]
fn folder_menu_keeps_both_directions_and_explains_empty_refusal() {
    let _locale = crate::test_locale::force("en");
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

/// A longer translation or a smaller fitted-menu cap must not clip a counted batch action.
#[gpui::test]
fn batch_action_labels_fit_every_locale(cx: &mut gpui::TestAppContext) {
    use moon_core::config::{Language, UiThemeMode};
    use moon_ui::{MoonMenuSize, MoonSize, MoonTheme};

    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for theme in [UiThemeMode::Dark, UiThemeMode::Light] {
            cx.update(|cx| {
                MoonTheme::install_config(
                    crate::startup::moon_theme_config_for_presentation(theme, 1.0),
                    cx,
                );
                let tokens = MoonTheme::active_tokens(cx);
                let tier = match MoonMenuSize::from_theme(&tokens) {
                    MoonMenuSize::Tier(tier) => tier,
                    MoonMenuSize::Custom { .. } => MoonSize::Sm,
                };
                let metrics = tier.control_metrics();
                // MoonUI dropdown/layout.rs: outer pads, border, row pads, check slot and gaps.
                let chrome = crate::design::ui_value(cx, 4.0) * 2.0
                    + 2.0
                    + crate::design::ui_value(cx, metrics.pad_x) * 2.0
                    + crate::design::ui_value(cx, 12.0)
                    + crate::design::ui_value(cx, metrics.gap) * 2.0;
                for key in [
                    "strat.menu_forget_all_cores",
                    "strat.menu_delete_empty_all_cores",
                    "strat.menu_delete_empty_folders",
                ] {
                    let label = rust_i18n::t!(key, n = 99999).to_string();
                    let text = crate::design::ui_text_width_zoomed(
                        cx,
                        &label,
                        metrics.font_size,
                        600.0,
                        true,
                    );
                    assert!(
                        text + chrome <= crate::design::ui_value(cx, super::MENU_MAX_WIDTH),
                        "batch caption {label} exceeds the fitted menu cap in {}",
                        language.code()
                    );
                }
            });
        }
    }
}
