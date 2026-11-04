//! The returning user's new-core card must explain the VPS without clipping its description.

use gpui::{Context, FontWeight, IntoElement, ParentElement, Render, Styled, Window, px};
use moon_core::config::{Language, UiThemeMode};
use moon_ui::{MoonSelectableCard, MoonTheme, v_flex};
use rust_i18n::t;

use super::option_body;
use crate::connect_wizard::MIN_SIZE;
use crate::design;

/// Render the production card body under the minimum-width start screen's prose and padding.
struct StartCardProbe;

impl Render for StartCardProbe {
    /// Match the wizard frame with its rail hidden below 640 design pixels.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .p(design::ui_px(cx, 22.0))
            .font_family(design::ui_font())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .child(
                MoonSelectableCard::new("cw-opt-new")
                    .selected(true)
                    .child(option_body(
                        t!("connect_wizard.start.new_title").to_string(),
                        t!("connect_wizard.start.new_more").to_string(),
                        None,
                        cx,
                    )),
            )
    }
}

/// Longer translations or a shrinking card must not hide the VPS requirements or add a fourth line.
#[gpui::test]
fn new_more_fits_three_lines_at_wizard_minimum_width(cx: &mut gpui::TestAppContext) {
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for theme in [
            UiThemeMode::Light,
            UiThemeMode::Dark,
            UiThemeMode::Graphite,
            UiThemeMode::LightExperimental,
            UiThemeMode::DarkExperimental,
        ] {
            cx.update(|app| {
                MoonTheme::install_config(
                    crate::startup::moon_theme_config_for_presentation(theme, 1.0),
                    app,
                );
            });
            let handle = cx.open_window(gpui::size(px(MIN_SIZE.0), px(MIN_SIZE.1)), |_, _| {
                StartCardProbe
            });
            let mut visual = gpui::VisualTestContext::from_window(handle.into(), cx);
            for _ in 0..3 {
                visual.update(|window, app| {
                    let _ = window.draw(app);
                });
            }
            let card = visual
                .debug_bounds("moon-selectable-card:cw-opt-new")
                .expect("card");
            assert!(
                card.origin.x >= px(0.0) && card.right() <= px(MIN_SIZE.0),
                "{language:?}/{theme:?}: card outside minimum-width window"
            );
            for (selector, key, weight) in [
                (
                    "cw-option-title",
                    "connect_wizard.start.new_title",
                    FontWeight::SEMIBOLD,
                ),
                (
                    "cw-option-description",
                    "connect_wizard.start.new_more",
                    FontWeight::NORMAL,
                ),
            ] {
                let text_box = visual.debug_bounds(selector).expect(selector);
                let text = t!(key).to_string();
                let (height, line_height) = visual.update(|window, app| {
                    let runs = [gpui::TextRun {
                        len: text.len(),
                        font: gpui::Font {
                            weight,
                            ..gpui::font(design::ui_font())
                        },
                        ..gpui::TextRun::default()
                    }];
                    let line_height = design::line_px(app, 14.0);
                    let height = window
                        .text_system()
                        .shape_text(
                            text.into(),
                            design::t_body(app),
                            &runs,
                            Some(text_box.size.width),
                            None,
                        )
                        .expect("card text shaping")
                        .iter()
                        .map(|line| f32::from(line.size(line_height).height))
                        .sum::<f32>();
                    (height, f32::from(line_height))
                });
                assert!(
                    height <= 3.0 * line_height + 0.5,
                    "{language:?}/{theme:?}/{selector}: text needs {} lines",
                    height / line_height
                );
                assert!(
                    height <= f32::from(text_box.size.height) + 0.5,
                    "{language:?}/{theme:?}/{selector}: text clipped"
                );
                assert!(
                    text_box.origin.x >= card.origin.x
                        && text_box.right() <= card.right()
                        && text_box.origin.y >= card.origin.y
                        && text_box.bottom() <= card.bottom(),
                    "{language:?}/{theme:?}/{selector}: text outside card"
                );
            }
            let title = visual.debug_bounds("cw-option-title").expect("title");
            let description = visual
                .debug_bounds("cw-option-description")
                .expect("description");
            assert!(
                title.bottom() <= description.origin.y,
                "{language:?}/{theme:?}: title overlaps description"
            );
        }
    }
}
