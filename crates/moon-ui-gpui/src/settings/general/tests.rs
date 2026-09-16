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

/// Catches omitting the font scale or scaling density's font delta twice during theme construction.
/// At 125% zoom, Standard's 10px base plus 3px density adjustment must render as 16.25px.
#[test]
fn density_and_zoom_scale_installed_text_proportionally() {
    for mode in [
        UiThemeMode::Dark,
        UiThemeMode::Graphite,
        UiThemeMode::Light,
        UiThemeMode::DarkExperimental,
        UiThemeMode::LightExperimental,
    ] {
        for (density, tier, text_px) in [
            (UiDensity::Compact, MoonSize::Xs, 12.5),
            (UiDensity::Standard, MoonSize::Sm, 16.25),
            (UiDensity::Large, MoonSize::Md, 20.0),
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

/// Catches `startup.rs:moon_theme_config_for_mode` installing a bundled palette theme for an
/// experimental mode, or putting it on the wrong side. The experimental modes must install MoonUI's
/// colour roles on the side the mode names, so checkboxes and radios paint the colour modes exactly.
#[test]
fn experimental_modes_install_moonui_colour_roles_on_their_own_side() {
    use moon_ui::{MoonColors, ThemeMode};
    for (mode, side, roles) in [
        (
            UiThemeMode::DarkExperimental,
            ThemeMode::Dark,
            MoonColors::DARK,
        ),
        (
            UiThemeMode::LightExperimental,
            ThemeMode::Light,
            MoonColors::LIGHT,
        ),
    ] {
        let theme = MoonTheme::from_config(crate::startup::moon_theme_config_for_mode(mode));
        assert_eq!(theme.config.mode, side);
        assert_eq!(theme.colors, Some(roles));
        assert_eq!(theme.palette, roles.to_palette());
    }
    for mode in [UiThemeMode::Dark, UiThemeMode::Graphite, UiThemeMode::Light] {
        let theme = MoonTheme::from_config(crate::startup::moon_theme_config_for_mode(mode));
        assert_eq!(
            theme.colors, None,
            "{mode:?} keeps its bundled palette theme"
        );
    }
}
