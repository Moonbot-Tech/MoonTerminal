//! Static contracts for the interface theme selector.

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

/// Catches `startup.rs:moon_theme_config_for_mode` mapping Obsidian to `moon_terminal()` or to
/// MoonUI's light side. A selected Obsidian mode must install the bundled obsidian palette while
/// retaining Dark mode.
#[test]
fn obsidian_startup_mapping_installs_the_obsidian_dark_theme() {
    let startup = code_only(&read_src("startup.rs"));
    let mapping = braced_body(
        &startup,
        "pub(crate) fn moon_theme_config_for_mode(mode: UiThemeMode)",
    );
    assert!(mapping.contains("UiThemeMode::Obsidian => MoonThemeConfig::moon_obsidian()"));
    assert!(mapping.contains("UiThemeMode::Obsidian => ThemeMode::Dark"));
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

/// Catches omitting a locale line in `locales/<lang>/interface.<lang>.yml` for the theme selector,
/// the Obsidian theme card, or the note naming the Obsidian chart colour set. A missing language
/// renders a raw locale key in Settings.
#[test]
fn graphite_selector_keys_define_each_shipped_language() {
    for key in [
        "iface.theme_mode",
        "iface.graphite_theme",
        "iface.obsidian_theme",
        "iface.theme.obsidian",
        "iface.set_note.obsidian",
        "iface.light_experimental_theme",
        "iface.dark_experimental_theme",
    ] {
        assert_locale_key_in_every_language("interface", key);
    }
}

/// The chart colour set has THREE tables — `[dark]`, `[light]` and Obsidian's own — and only the
/// interface MODE can name the third. Every place that picks the chart theme to draw or to edit
/// must therefore go through `AppConfig::chart_theme` / `chart_theme_mut`, never through the
/// set's light/dark accessor.
///
/// Breakage: `cfg.theme.get(palette.is_light())` or `cfg.theme.get_mut(mode.is_light())`
/// compiles, reads correctly for five of the six modes, and hands Obsidian the dark table — the
/// chart draws a grey ground inside near-black panels, or the Appearance pickers edit a table
/// that theme never reads. Nothing else goes red, which is why the ban is crate-wide rather than
/// per file.
#[test]
fn the_chart_theme_is_picked_by_interface_mode_everywhere() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_sources(&root, &mut files);
    for path in files {
        // A test may ask for one table by name; the rule is about what the application draws.
        if file_name(&path) == "tests.rs" {
            continue;
        }
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        // Whitespace is dropped as well as comments: rustfmt breaks a long chain so that
        // `.theme` and `.get(` sit on separate lines, and the ban must still see them as one.
        let source: String = code_only(&text).split_whitespace().collect();
        for banned in [".theme.get(", ".theme.get_mut("] {
            assert!(
                !source.contains(banned),
                "{} must pick the chart theme through `chart_theme()` / `chart_theme_mut()`, \
                 not `{banned}`: a light/dark answer cannot reach the `[obsidian]` table",
                path.display()
            );
        }
    }

    // The three surfaces that used to ask the palette or `is_light()` must name the accessor, so
    // deleting the theme from one of them is not mistaken for satisfying the ban above.
    let render = code_only(&read_src("panels/chart/render.rs"));
    assert!(
        render.contains("eff.chart_theme()"),
        "panels/chart/render.rs must hand the engine the effective config's mode-chosen theme"
    );
    let stack = code_only(&read_src("chart_tabs/add_stack.rs"));
    assert!(
        stack.contains("effective.chart_theme().bg"),
        "chart_tabs/add_stack.rs must measure the arrival colour against the ground the chart draws"
    );
    let appearance = code_only(&read_src("settings/interface.rs"));
    assert!(
        appearance.contains(".chart_theme_mut()") && appearance.contains(".chart_theme()"),
        "settings/interface.rs must read and write the table the chart draws"
    );
    // The note beside the swatches is derived from the same set, with an arm for the third table.
    let note = braced_body(&appearance, "fn set_note_key(mode: UiThemeMode)");
    assert!(
        note.contains("ChartColorSet::for_mode(mode)")
            && note.contains("ChartColorSet::Obsidian => \"iface.set_note.obsidian\""),
        "settings/interface.rs:set_note_key must name the Obsidian chart set under Obsidian"
    );
}
