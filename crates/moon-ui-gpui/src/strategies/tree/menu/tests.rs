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

/// A longer translation or a smaller fitted-menu cap must not clip the new batch action.
#[gpui::test]
fn forget_all_cores_label_fits_every_locale(cx: &mut gpui::TestAppContext) {
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
                let label = rust_i18n::t!("strat.menu_forget_all_cores", n = 99999).to_string();
                let text =
                    crate::design::ui_text_width_zoomed(cx, &label, metrics.font_size, 600.0, true);
                // MoonUI dropdown/layout.rs: outer pads, border, row pads, check slot and gaps.
                let chrome = crate::design::ui_value(cx, 4.0) * 2.0
                    + 2.0
                    + crate::design::ui_value(cx, metrics.pad_x) * 2.0
                    + crate::design::ui_value(cx, 12.0)
                    + crate::design::ui_value(cx, metrics.gap) * 2.0;
                assert!(
                    text + chrome <= crate::design::ui_value(cx, super::MENU_MAX_WIDTH),
                    "batch forget caption exceeds the fitted menu cap in {}",
                    language.code()
                );
            });
        }
    }
}
