//! Appearance page: theme picking, the edited colour set, and the layout.

use gpui::{AppContext as _, IntoElement};
use moon_core::config::{AppConfig, UiThemeMode};

use super::{
    AppearanceParts, ColorGroup, appearance_body, chart_preview, color_line, pick_theme,
    set_note_key, slider_ctl, theme_card_meta, visible_modes,
};
use crate::settings::general::probe_host::{all_open, host, stub};
use crate::settings::layout_probe::{ProbeSpec, sweep};

const ALL_MODES: [UiThemeMode; 5] = [
    UiThemeMode::Light,
    UiThemeMode::Graphite,
    UiThemeMode::Dark,
    UiThemeMode::LightExperimental,
    UiThemeMode::DarkExperimental,
];

/// Catches a card wired to the wrong mode: the Graphite card writes Graphite, a second click
/// changes nothing, and every card id is distinct.
#[test]
fn the_graphite_card_writes_graphite() {
    let mut config = AppConfig::load(None, false).expect("test-binary config must load");
    config.ui_theme_mode = UiThemeMode::Dark;
    assert!(pick_theme(&mut config, UiThemeMode::Graphite));
    assert_eq!(config.ui_theme_mode, UiThemeMode::Graphite);
    assert!(
        !pick_theme(&mut config, UiThemeMode::Graphite),
        "a repeat is not a change"
    );

    let mut ids: Vec<_> = ALL_MODES.map(|mode| theme_card_meta(mode).0).to_vec();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), ALL_MODES.len(), "card ids must be distinct");
}

/// Catches the experimental themes leaking into the default row, or the current theme's card
/// vanishing: hidden until the box is checked, unless one of them is already in use.
#[test]
fn experimental_cards_stay_hidden_until_checked() {
    let experimental = [
        UiThemeMode::LightExperimental,
        UiThemeMode::DarkExperimental,
    ];
    let hidden = visible_modes(false, UiThemeMode::Dark);
    assert_eq!(
        hidden,
        vec![UiThemeMode::Light, UiThemeMode::Graphite, UiThemeMode::Dark]
    );
    let shown = visible_modes(true, UiThemeMode::Dark);
    assert!(experimental.iter().all(|mode| shown.contains(mode)));
    for current in experimental {
        assert!(
            visible_modes(false, current).contains(&current),
            "{current:?} in use must keep its card"
        );
    }
}

/// Catches the colour-set note drifting from the set the editors write: it follows `is_light()`
/// for all five modes.
#[test]
fn the_set_note_follows_is_light_for_every_mode() {
    for mode in ALL_MODES {
        let want = if mode.is_light() {
            "iface.set_note.light"
        } else {
            "iface.set_note.dark"
        };
        assert_eq!(set_note_key(mode), want, "{mode:?}");
    }
}

/// The Appearance page with every help unfolded keeps the theme row, zoom, colour groups,
/// preview and the book box inside the body and apart, without cutting the set note or the
/// unfolded help, in every language at the default and the minimum window.
#[gpui::test]
fn appearance_page_fits_the_body(cx: &mut gpui::TestAppContext) {
    let groups = [
        "iface-grp-bg",
        "iface-grp-candles",
        "iface-grp-price",
        "iface-grp-trades",
        "iface-grp-book",
        "iface-grp-labels",
    ];
    sweep(
        cx,
        host(move |_, cx| {
            let theme = AppConfig::load(None, false)
                .expect("test-binary config must load")
                .theme
                .get(false)
                .clone();
            let row = |id: &'static str| stub(id, 240.0, 24.0);
            let parts = AppearanceParts {
                cards: ALL_MODES
                    .into_iter()
                    .map(|mode| stub(theme_card_meta(mode).0, 150.0, 80.0))
                    .collect(),
                experimental: stub("probe-experimental", 220.0, 18.0),
                zoom: stub("probe-zoom", 180.0, 22.0),
                set_note: rust_i18n::t!("iface.set_note.dark").to_string(),
                groups: groups
                    .map(|id| ColorGroup {
                        id,
                        title: id.to_string(),
                        rows: vec![row("probe-row-a"), row("probe-row-b")],
                    })
                    .into_iter()
                    .collect(),
                preview: chart_preview(&theme, cx),
                book_width: stub("probe-book-width", 130.0, 28.0),
                dock_reset: stub("probe-dock-reset", 200.0, 28.0),
            };
            appearance_body(parts, &all_open, cx).into_any_element()
        }),
        move |_| {
            let mut inside: Vec<String> = ALL_MODES
                .map(|mode| theme_card_meta(mode).0.to_string())
                .to_vec();
            inside.extend(groups.map(String::from));
            inside.extend(
                [
                    "iface-themes",
                    "iface-zoom",
                    "iface-colors",
                    "iface-preview",
                    "iface-book-width",
                    "iface-dock-reset",
                    "probe-dock-reset",
                ]
                .map(String::from),
            );
            let mut apart: Vec<String> = groups.map(String::from).to_vec();
            apart.push("iface-preview".into());
            ProbeSpec {
                container: "probe-body",
                inside,
                no_overlap: vec![
                    apart,
                    [
                        "iface-themes",
                        "iface-zoom",
                        "iface-colors",
                        "iface-book-width",
                    ]
                    .map(String::from)
                    .to_vec(),
                ],
                wraps: vec![
                    (
                        "iface-set-note".into(),
                        rust_i18n::t!("iface.set_note.dark").to_string(),
                    ),
                    (
                        "iface-zoom-more-text".into(),
                        rust_i18n::t!("iface.ui_zoom_hint").to_string(),
                    ),
                    (
                        "iface-dock-reset-more-text".into(),
                        rust_i18n::t!("iface.dock_reset_tip").to_string(),
                    ),
                    (
                        "iface-footnote".into(),
                        rust_i18n::t!("iface.hint").to_string(),
                    ),
                ],
                last: Some("iface-footnote".into()),
                ..Default::default()
            }
        },
    );
}

/// The longest colour labels of the Appearance groups, each probed with and without the opacity
/// slider beside it.
const LONG_COLOR_LABELS: [&str; 4] = [
    "iface.panel_bg",
    "iface.hvol_bg",
    "iface.candle_neutral",
    "iface.price_line",
];

/// Catches a colour label painting past its card ("Фон панелей (тулбар, ордер…" over the border):
/// the label must lie inside its group and be tall enough for its wrapped text, in every
/// language at the default and the minimum window, beside a slider or alone.
#[gpui::test]
fn colour_labels_wrap_inside_their_card(cx: &mut gpui::TestAppContext) {
    for key in LONG_COLOR_LABELS {
        for with_slider in [false, true] {
            sweep(
                cx,
                host(move |window, cx| {
                    let theme = AppConfig::load(None, false)
                        .expect("test-binary config must load")
                        .theme
                        .get(false)
                        .clone();
                    let picker = cx.new(|cx| moon_ui::MoonColorPickerState::new(window, cx));
                    let slider = cx.new(|_| moon_ui::MoonSliderState::new().max(100.0).min(0.0));
                    let p = moon_ui::MoonPalette::active(cx);
                    let label = rust_i18n::t!(key).to_string();
                    let mut row = Some(color_line(
                        crate::settings::color_row(&label, &picker, p, cx).into_any_element(),
                        with_slider.then(|| slider_ctl(&slider, &|v| format!("{v:.0}%"), cx)),
                        cx,
                    ));
                    let parts = AppearanceParts {
                        cards: Vec::new(),
                        experimental: stub("probe-experimental", 220.0, 18.0),
                        zoom: stub("probe-zoom", 180.0, 22.0),
                        set_note: String::new(),
                        groups: ["iface-grp-bg", "iface-grp-candles", "iface-grp-price"]
                            .map(|id| ColorGroup {
                                id,
                                title: id.to_string(),
                                rows: vec![
                                    row.take().unwrap_or_else(|| stub("probe-row", 240.0, 24.0)),
                                ],
                            })
                            .into_iter()
                            .collect(),
                        preview: chart_preview(&theme, cx),
                        book_width: stub("probe-book-width", 130.0, 28.0),
                        dock_reset: stub("probe-dock-reset", 200.0, 28.0),
                    };
                    appearance_body(parts, &all_open, cx).into_any_element()
                }),
                move |_| ProbeSpec {
                    container: "iface-grp-bg",
                    inside: vec!["color-row-label".into()],
                    wraps: vec![("color-row-label".into(), rust_i18n::t!(key).to_string())],
                    ..Default::default()
                },
            );
        }
    }
}
