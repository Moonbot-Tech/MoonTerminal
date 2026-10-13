//! Layout coverage for the longest localized figure actions under the production menu route.

use moon_core::config::{Language, UiThemeMode};
use moon_ui::MoonTheme;
use rust_i18n::t;

/// Reverting the figure menu to 200px clips share/delete-shared actions in long locales.
#[gpui::test]
fn figure_menu_labels_fit_every_locale(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    for mode in [
        UiThemeMode::Dark,
        UiThemeMode::Light,
        UiThemeMode::Graphite,
        UiThemeMode::LightExperimental,
        UiThemeMode::DarkExperimental,
    ] {
        cx.update(|app| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(mode, 1.0),
                app,
            );
        });
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            // Superset of the real figure's conditional branches: includes both longest
            // alternatives so a state change cannot hide a locale's worst case.
            let labels = [
                "chart.fig_menu.settings",
                "chart.fig_menu.sells_to_zone",
                "chart.fig_menu.alert_on",
                "chart.fig_menu.alert_off",
                "chart.fig_menu.share",
                "chart.fig_menu.unshare",
                "chart.fig_menu.delete",
                "chart.fig_menu.delete_shared",
            ]
            .map(|key| t!(key).to_string())
            .to_vec();
            crate::panels::tab_menu::tests::assert_menu_fits(
                cx,
                "chart-fig-menu",
                labels,
                super::open_figure_menu,
            );
        }
    }
}
