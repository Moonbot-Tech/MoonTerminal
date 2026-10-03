//! Static contracts for the three-entry interface theme selector.

use super::support::*;

/// Catches `startup.rs:moon_theme_config_for_mode` mapping Graphite to `moon_terminal()`.
/// A selected Graphite mode must install the bundled graphite palette while retaining Dark mode.
#[test]
fn graphite_startup_mapping_installs_the_graphite_dark_theme() {
    let startup = code_only(&read_src("startup.rs"));
    let mapping = braced_body(
        &startup,
        "pub(crate) fn moon_theme_config_for_mode(mode: UiThemeMode)",
    );
    assert!(mapping.contains("UiThemeMode::Graphite => MoonThemeConfig::moon_graphite()"));
    assert!(mapping.contains("UiThemeMode::Dark | UiThemeMode::Graphite => ThemeMode::Dark"));
}

/// Catches `startup/unlock.rs:install_login_theme` returning to a separate two-arm mapping.
/// Login and the running application must share the mode and zoom presentation mapping.
#[test]
fn login_theme_reuses_the_application_theme_mapping_for_all_three_modes() {
    let unlock = code_only(&read_src("startup/unlock.rs"));
    let install = braced_body(&unlock, "fn install_login_theme(cx: &mut App)");
    assert!(install.contains("moon_theme_config_for_presentation("));
    for preference in ["prefs.ui_theme_mode", "prefs.ui_scale"] {
        assert!(install.contains(preference));
    }
    let startup = code_only(&read_src("startup.rs"));
    let application = braced_body(&startup, "pub(crate) fn moon_theme_config_for(cfg:");
    assert!(application.contains("moon_theme_config_for_presentation("));
    assert!(!install.contains("UiThemeMode::Light => moon_ui::MoonThemeConfig::moon_light()"));
}

/// Catches omitting a locale line in `locales/<lang>/interface.<lang>.yml` for the theme selector.
/// A missing language renders a raw locale key on the General settings tab.
#[test]
fn graphite_selector_keys_define_each_shipped_language() {
    for key in [
        "iface.theme_mode",
        "iface.graphite_theme",
        "iface.light_experimental_theme",
        "iface.dark_experimental_theme",
    ] {
        assert_locale_key_in_every_language("interface", key);
    }
}
