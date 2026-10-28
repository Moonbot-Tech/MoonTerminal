//! Appearance page: theme picking, the edited colour set, and the layout.

use gpui::{
    AppContext as _, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _,
};
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
                        "iface-dock-reset",
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

/// Catches the interface-scale slider pinned to the far edge with a hole across its card: in
/// every language at every swept size and a wide window, the slider starts right after the
/// label column — one row gap from the label's right edge, or under the label when the row
/// wrapped — and the label column is no wider than its 240 cap.
#[gpui::test]
fn the_zoom_slider_sits_beside_its_label(cx: &mut gpui::TestAppContext) {
    use crate::settings::layout_probe::{SIZES, body_box, leak};
    cx.update(moon_ui::init);
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for size in SIZES.into_iter().chain([(1940.0, 1180.0)]) {
            let at = format!("{} at {}x{}", language.code(), size.0, size.1);
            let make = host(|_, cx| {
                let parts = AppearanceParts {
                    cards: Vec::new(),
                    experimental: stub("probe-experimental", 220.0, 18.0),
                    zoom: stub("probe-zoom", 300.0, 22.0),
                    set_note: String::new(),
                    groups: Vec::new(),
                    preview: stub("probe-preview", 300.0, 200.0),
                    dock_reset: stub("probe-dock-reset", 200.0, 28.0),
                };
                appearance_body(parts, &all_open, cx).into_any_element()
            });
            let window = cx.open_window(gpui::size(gpui::px(size.0), gpui::px(size.1)), |w, cx| {
                let body = body_box(size, cx);
                make(w, cx, body)
            });
            let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
            for _ in 0..3 {
                visual.update(|window, cx| {
                    let _ = window.draw(cx);
                });
            }
            let (gap, cap) = visual.update(|_, cx| {
                (
                    crate::design::ui_value(cx, 16.0),
                    crate::design::ui_value(cx, crate::settings::common::HINT_LABEL_MAX_W),
                )
            });
            let mut get = |sel: &str| {
                visual
                    .debug_bounds(leak(sel.to_string()))
                    .unwrap_or_else(|| panic!("{sel} was not laid out ({at})"))
            };
            let label = get("iface-zoom-label");
            let slider = get("probe-zoom");
            assert!(
                f32::from(label.size.width) <= cap + 0.5,
                "zoom label column is {} wide, over the {cap} cap ({at})",
                f32::from(label.size.width)
            );
            let beside = slider.origin.y < label.bottom_left().y;
            if beside {
                let hole = f32::from(slider.origin.x - label.right());
                assert!(
                    (hole - gap).abs() <= 0.5,
                    "zoom slider is {hole} px from its label column, not one {gap} gap ({at})"
                );
            } else {
                let dx = f32::from(slider.origin.x - label.origin.x).abs();
                assert!(
                    dx <= 0.5,
                    "wrapped zoom slider is not under its label ({at})"
                );
            }
        }
    }
}

/// Catches a colour-group slider pinned to the far edge of a wide group: in a 900-wide group the
/// slider follows the label column, which stops at its cap, and the value sits right after the
/// track.
#[gpui::test]
fn a_group_slider_follows_its_label(cx: &mut gpui::TestAppContext) {
    use crate::settings::layout_probe::leak;
    cx.update(moon_ui::init);
    for language in moon_core::config::Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let window = cx.open_window(
            gpui::size(gpui::px(1000.0), gpui::px(300.0)),
            |window, cx| {
                let slider = cx.new(|_| moon_ui::MoonSliderState::new().max(100.0).min(0.0));
                crate::settings::general::probe_host::host(move |_, cx| {
                    gpui::div()
                        .w(gpui::px(900.0))
                        .debug_selector(|| "probe-group".into())
                        .child(color_line(
                            gpui::div()
                                .debug_selector(|| "probe-label".into())
                                .child(rust_i18n::t!("iface.book_level_alpha").to_string())
                                .into_any_element(),
                            Some(
                                gpui::div()
                                    .debug_selector(|| "probe-slider".into())
                                    .child(slider_ctl(&slider, &|v| format!("{v:.0}%"), cx))
                                    .into_any_element(),
                            ),
                            cx,
                        ))
                        .into_any_element()
                })(window, cx, (950.0, 280.0))
            },
        );
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        for _ in 0..3 {
            visual.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        let group = visual
            .debug_bounds(leak("probe-group".into()))
            .expect("group");
        let slider = visual
            .debug_bounds(leak("probe-slider".into()))
            .expect("slider");
        let width = f32::from(group.size.width);
        let start = f32::from(slider.origin.x - group.origin.x);
        assert!(
            start <= 0.5 * width,
            "slider starts {start} px into a {width} group: pinned to the far edge ({})",
            language.code()
        );
    }
}
