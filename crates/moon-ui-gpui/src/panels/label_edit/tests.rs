//! Selection safety and the production module dialog's localized layout.

use super::clamped;

use gpui::{AppContext as _, Context, IntoElement, Render, Window, px};
use moon_core::config::{
    ChartLabelField, ChartLabelRow, LabelColor, LabelFlow, LabelSpan, Language, PnlBasis,
    SpanAnchor, UiThemeMode, VolumeUnits,
};
use moon_ui::MoonTheme;
use rust_i18n::t;

/// Inert host for the real Root-owned dialog, with no backend or persistent state.
struct Host;

impl Render for Host {
    /// Leave all dialog geometry to the production opener and MoonUI Root.
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::div()
    }
}

/// Read one instrumented production element without limiting selectors to string literals.
fn bounds(visual: &mut gpui::VisualTestContext, selector: String) -> gpui::Bounds<gpui::Pixels> {
    visual
        .debug_bounds(Box::leak(selector.into_boxed_str()))
        .expect("element laid out")
}

/// Independent full-label requirements, including choices on captions beyond the cursor preset.
fn choices(field: ChartLabelField) -> Vec<(&'static str, Vec<String>)> {
    let mut rows = vec![
        (
            "le-flow",
            LabelFlow::ALL
                .iter()
                .map(|v| t!(v.locale_key()).to_string())
                .collect(),
        ),
        (
            "le-size",
            ["0.75", "1", "1.25", "1.45", "1.5", "1.7", "2"]
                .map(str::to_string)
                .to_vec(),
        ),
        (
            "le-color",
            [
                "chart_labels.color_theme",
                "chart_labels.color_sign",
                "chart_labels.color_fixed",
            ]
            .map(|k| t!(k).to_string())
            .to_vec(),
        ),
    ];
    if field.uses_pnl_basis() {
        rows.push((
            "le-basis",
            PnlBasis::ALL
                .iter()
                .map(|v| t!(v.locale_key()).to_string())
                .collect(),
        ));
    }
    if field.in_volume_block() {
        rows.push((
            "le-anchor",
            SpanAnchor::ALL
                .iter()
                .map(|v| t!(v.locale_key()).to_string())
                .collect(),
        ));
    }
    if field.uses_volume_units() {
        rows.push((
            "le-units",
            VolumeUnits::ALL
                .iter()
                .map(|v| t!(v.locale_key()).to_string())
                .collect(),
        ));
    }
    if field.is_percent() {
        rows.push((
            "le-color-min",
            vec![
                t!("chart_labels.color_min_off").to_string(),
                "0.1%".into(),
                "0.25%".into(),
                "0.5%".into(),
                "1%".into(),
                "2%".into(),
            ],
        ));
    }
    rows
}

/// Reverting any fixed-width choice or period trigger, or removing Reset wrapping, must fail
/// against full text at the shipped +3 font delta in the actual 600px dialog.
#[gpui::test]
fn module_editor_labels_fit_every_locale(cx: &mut gpui::TestAppContext) {
    crate::test_locale::warm();
    cx.update(moon_ui::init);
    for mode in [UiThemeMode::Light, UiThemeMode::Dark] {
        cx.update(|app| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(mode, 1.0),
                app,
            )
        });
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            for field in ChartLabelField::ALL {
                let (_, visual) = cx.add_window_view(|window, cx| {
                    let host = cx.new(|_| Host);
                    moon_ui::Root::new(host, window, cx).bordered(false)
                });
                visual.simulate_resize(gpui::size(px(900.0), px(1400.0)));
                visual.update(|window, app| {
                    let mut row = ChartLabelRow::new(
                        ChartLabelRow::DEFAULT_ZONE,
                        ChartLabelRow::DEFAULT_ALIGN,
                    );
                    row.push_part(field);
                    row.parts[0].span = LabelSpan::Trades(1000);
                    row.parts[0].style.color = Some(LabelColor::BySign);
                    super::open_label_edit(
                        "Module".into(),
                        row,
                        60_000,
                        window,
                        app,
                        |_, _| {},
                        |_| {},
                        |_, _| {},
                    );
                });
                for _ in 0..3 {
                    visual.update(|window, app| {
                        let _ = window.draw(app);
                    });
                }
                let pane = bounds(visual, "le-settings-content".into());
                let frame = bounds(visual, "le-settings".into());
                let at = format!("{} {mode:?} {field:?}", language.code());
                for id in ["le-caption-check", "le-reset"] {
                    let cell = bounds(visual, id.into());
                    assert!(
                        cell.origin.x >= pane.origin.x && cell.right() <= pane.right() + px(0.5),
                        "{id} crosses group: {at}"
                    );
                    assert!(
                        cell.origin.y >= frame.origin.y && cell.bottom() <= frame.bottom(),
                        "{id} crosses group vertically: {at}"
                    );
                }
                let check = bounds(visual, "le-caption-check".into());
                let reset = bounds(visual, "le-reset".into());
                assert!(
                    !check.intersects(&reset),
                    "checkbox and Reset overlap: {at}"
                );
                for (id, labels) in choices(field) {
                    let mut cells = Vec::new();
                    for (ix, label) in labels.iter().enumerate() {
                        let cell = bounds(visual, format!("{id}-cell-{ix}"));
                        let width = visual.update(|_, app| {
                            crate::design::ui_text_width(app, label, 11.0, 500.0, true)
                        });
                        assert!(
                            cell.size.width + px(0.5) >= px(width + 16.0),
                            "full label {label} clips: {at}"
                        );
                        if id != "le-flow" {
                            assert!(
                                cell.origin.x >= pane.origin.x
                                    && cell.right() <= pane.right() + px(0.5),
                                "{id} escapes pane: {at}"
                            );
                        }
                        for prior in &cells {
                            assert!(!cell.intersects(prior), "{id} sibling overlap: {at}");
                        }
                        cells.push(cell);
                    }
                }
                if field.uses_window() {
                    let trigger = bounds(visual, "le-window-trigger".into());
                    let label = t!("chart_labels.span.trades", n = 1000).to_string();
                    let width = visual.update(|_, app| {
                        crate::design::ui_text_width(
                            app,
                            &format!("{label} \u{25be}"),
                            14.0,
                            400.0,
                            true,
                        )
                    });
                    assert!(
                        trigger.size.width + px(0.5) >= px(width + 14.0),
                        "1000 trades clips: {at}"
                    );
                    assert!(
                        trigger.origin.x >= pane.origin.x
                            && trigger.right() <= pane.right() + px(0.5),
                        "period escapes pane: {at}"
                    );
                }
                visual.update(|window, _| window.remove_window());
            }
        }
    }
}

/// Removing captions must not leave the settings pane pointing past the list — the pane reads
/// `row.parts[selected]`, and a selection past the end would describe a caption that is gone.
#[test]
fn the_selection_follows_a_shrinking_list() {
    assert_eq!(clamped(3, 4), 3, "a valid selection is left alone");
    assert_eq!(
        clamped(3, 2),
        1,
        "a removal pulls it back to the last caption"
    );
    assert_eq!(
        clamped(0, 0),
        0,
        "an empty module selects where the first caption will land"
    );
}
