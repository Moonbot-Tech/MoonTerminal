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
/// Login and the running application must use one theme-mapping authority for every mode.
#[test]
fn login_theme_reuses_the_application_theme_mapping_for_all_three_modes() {
    let unlock = code_only(&read_src("startup/unlock.rs"));
    let install = braced_body(&unlock, "fn install_login_theme(cx: &mut App)");
    assert!(install.contains("moon_theme_config_for_mode(prefs.ui_theme_mode)"));
    assert!(!install.contains("UiThemeMode::Light => moon_ui::MoonThemeConfig::moon_light()"));
}

/// Catches omitting a locale line in `locales/interface.yml` for the theme selector.
/// A missing language renders a raw locale key on the General settings tab.
#[test]
fn graphite_selector_keys_define_each_shipped_language() {
    let locale = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../locales/interface.yml"),
    )
    .expect("Interface locales must be readable")
    .replace("\r\n", "\n");
    for key in ["iface.theme_mode", "iface.graphite_theme"] {
        let members: Vec<&str> = locale
            .split_once(&format!("{key}:\n"))
            .unwrap_or_else(|| panic!("interface locale must define {key}"))
            .1
            .lines()
            .take_while(|line| line.starts_with("  "))
            .collect();
        assert_eq!(members.len(), 3, "{key} must define exactly ru, en, and es");
        for language in ["ru", "en", "es"] {
            assert!(
                members
                    .iter()
                    .any(|line| line.starts_with(&format!("  {language}: "))),
                "{key} must define {language}, or the raw key reaches Settings"
            );
        }
    }
}
