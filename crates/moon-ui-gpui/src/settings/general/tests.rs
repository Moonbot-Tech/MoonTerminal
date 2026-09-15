//! Density presentation and zoom-caption regressions for the General tab.

use super::zoom_text;
use moon_core::config::{UiDensity, UiThemeMode};
use moon_ui::{MoonSize, MoonTheme};

/// Catches removing percentage conversion or leaking floating step noise into zoom captions.
#[test]
fn zoom_captions_show_percentages_at_endpoints_and_steps() {
    for (scale, text) in [(0.75, "75%"), (1.0, "100%"), (1.05, "105%"), (1.50, "150%")] {
        assert_eq!(zoom_text(scale), text);
    }
}

/// Catches omitting density from theme construction or losing the +3 Standard text adjustment.
/// The installed tokens must keep existing 10px text at 13px while zoom changes geometry only.
#[test]
fn density_reaches_both_theme_modes_without_zooming_text_twice() {
    for mode in [UiThemeMode::Dark, UiThemeMode::Graphite, UiThemeMode::Light] {
        for (density, tier, text_px) in [
            (UiDensity::Compact, MoonSize::Xs, 10.0),
            (UiDensity::Standard, MoonSize::Sm, 13.0),
            (UiDensity::Large, MoonSize::Md, 16.0),
        ] {
            let theme = MoonTheme::from_config(crate::startup::moon_theme_config_for_presentation(
                mode, density, 1.25,
            ));
            let tokens = if mode.is_light() {
                &theme.config.light
            } else {
                &theme.config.dark
            };
            assert_eq!(tokens.tier(), tier);
            assert_eq!(tokens.font(10.0), text_px);
            assert_eq!(tokens.ui(20.0), 25.0);
        }
    }
}
