//! Zoom presentation and zoom-caption regressions for the General tab.

use super::zoom_text;
use moon_core::config::UiThemeMode;
use moon_ui::{MoonSize, MoonTheme};

/// Catches removing percentage conversion or leaking floating step noise into zoom captions.
#[test]
fn zoom_captions_show_percentages_at_endpoints_and_steps() {
    for (scale, text) in [(0.5, "50%"), (1.0, "100%"), (1.05, "105%"), (2.0, "200%")] {
        assert_eq!(zoom_text(scale), text);
    }
}

/// Catches the slider's scale reaching MoonUI's token multipliers again instead of the window
/// content zoom: at 125% the tokens must still hand every design value through unscaled, or
/// the window would scale twice.
#[test]
fn zoom_is_installed_on_the_window_not_the_tokens() {
    for mode in [
        UiThemeMode::Dark,
        UiThemeMode::Graphite,
        UiThemeMode::Light,
        UiThemeMode::DarkExperimental,
        UiThemeMode::LightExperimental,
    ] {
        let theme = MoonTheme::from_config(crate::startup::moon_theme_config_for_presentation(
            mode, 1.25,
        ));
        let tokens = if mode.is_light() {
            &theme.config.light
        } else {
            &theme.config.dark
        };
        assert_eq!(tokens.zoom(), 1.25);
        assert_eq!(tokens.tier(), MoonSize::Sm);
        assert_eq!(tokens.font(10.0), 13.0);
        assert_eq!(tokens.ui(20.0), 20.0);
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
