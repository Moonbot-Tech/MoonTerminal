//! Zoom presentation and zoom-caption regressions, and the General page layout.

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

/// The General page with every long help unfolded keeps its three sections apart, every row and
/// control inside the body, and no unfolded help cut off, in every language at the default and
/// the minimum window.
#[gpui::test]
fn general_page_fits_the_body(cx: &mut gpui::TestAppContext) {
    use super::probe_host::{all_open, host, stub};
    use super::{GENERAL_ROWS, GeneralControls, general_body};
    use crate::settings::layout_probe::{ProbeSpec, sweep};

    sweep(
        cx,
        host(|_, cx| {
            let controls = GeneralControls {
                language: stub("probe-language", 220.0, 28.0),
                valuation: stub("probe-valuation", 260.0, 28.0),
                tab_per_core: stub("probe-split", 18.0, 18.0),
                book_pan: stub("probe-pan", 18.0, 18.0),
                idle_on: stub("probe-idle-on", 18.0, 18.0),
                idle_secs: stub("probe-idle-secs", 130.0, 28.0),
                log_files: stub("probe-logf", 18.0, 18.0),
                retention: stub("probe-retention", 130.0, 28.0),
            };
            general_body(controls, &all_open, cx)
        }),
        |_| {
            let mut inside: Vec<String> = GENERAL_ROWS.iter().map(|id| id.to_string()).collect();
            inside.extend(
                [
                    "probe-language",
                    "probe-valuation",
                    "probe-idle-secs",
                    "probe-retention",
                ]
                .map(String::from),
            );
            let wraps = [
                ("general-valuation", "general.valuation.more"),
                ("general-tab-per-core", "general.charts_split_by_core_hint"),
                ("general-book-pan", "general.book_pan.more"),
                ("general-idle", "general.main_idle_close_hint"),
                ("general-retention", "general.log_retention_hint"),
            ]
            .map(|(id, key)| (format!("{id}-more-text"), rust_i18n::t!(key).to_string()))
            .to_vec();
            ProbeSpec {
                container: "probe-body",
                inside,
                no_overlap: vec![
                    ["general-sec-lang", "general-sec-charts", "general-sec-logs"]
                        .map(String::from)
                        .to_vec(),
                    GENERAL_ROWS.map(String::from).to_vec(),
                ],
                wraps,
                last: Some("general-sec-logs".into()),
                ..Default::default()
            }
        },
    );
}

/// In a wide window every General label sits on one line: each section's label column grows to
/// its longest label instead of stopping at 240 and wrapping «Отдельная чарт-вкладка на / каждое
/// ядро» while the card's right side stays empty. Every language.
#[gpui::test]
fn general_labels_fit_one_line_in_a_wide_window(cx: &mut gpui::TestAppContext) {
    use super::probe_host::{all_open, host, stub};
    use super::{GENERAL_ROWS, GeneralControls, general_body};
    use crate::settings::layout_probe::{ProbeSpec, TextFace, probe_once};

    let keys = [
        "general.language",
        "general.valuation.label",
        "general.charts_split_by_core",
        "general.book_pan.label",
        "general.idle_close.label",
        "general.log_files.label",
        "general.log_retention.label",
    ];
    cx.update(moon_ui::init);
    let make = host(|_, cx| {
        let controls = GeneralControls {
            language: stub("probe-language", 220.0, 28.0),
            valuation: stub("probe-valuation", 260.0, 28.0),
            tab_per_core: stub("probe-split", 18.0, 18.0),
            book_pan: stub("probe-pan", 18.0, 18.0),
            idle_on: stub("probe-idle-on", 18.0, 18.0),
            idle_secs: stub("probe-idle-secs", 130.0, 28.0),
            log_files: stub("probe-logf", 18.0, 18.0),
            retention: stub("probe-retention", 130.0, 28.0),
        };
        general_body(controls, &all_open, cx)
    });
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let fits: Vec<_> = GENERAL_ROWS
            .iter()
            .zip(keys)
            .map(|(id, key)| {
                (
                    format!("{id}-label"),
                    rust_i18n::t!(key).to_string(),
                    TextFace::Ui,
                )
            })
            .collect();
        // The brief's cap is about 380 design px: every label up to it must fit one line, whatever
        // the column's own cap is (a longer one may wrap by design).
        let cap = cx.update(|cx| crate::design::ui_value(cx, 380.0));
        let fits: Vec<_> = fits
            .into_iter()
            .filter(|(_, text, _): &(String, String, TextFace)| {
                cx.update(|cx| crate::design::ui_body_text_width(cx, text, 400.0)) <= cap
            })
            .collect();
        let spec = ProbeSpec {
            container: "probe-body",
            fits,
            ..Default::default()
        };
        if let Err(broken) = probe_once(cx, (1900.0, 1000.0), &make, &spec) {
            panic!("{broken} ({} at 1900x1000)", language.code());
        }
    }
}
