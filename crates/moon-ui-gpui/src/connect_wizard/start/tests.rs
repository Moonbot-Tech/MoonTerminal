//! The returning user's new-core card must explain the VPS without clipping its description, and
//! its demo tick must leave the rest of its line to the card.

use gpui::{Context, FontWeight, IntoElement, ParentElement, Render, Styled, Window, px};
use moon_core::config::{Language, UiThemeMode};
use moon_ui::{MoonCheckbox, MoonSelectableCard, MoonTheme, v_flex};
use rust_i18n::t;

use super::{demo_tick, option_body};
use crate::connect_wizard::{DEFAULT_SIZE, MIN_SIZE};
use crate::design;

/// The new-core card as the start screen draws it: its text, then the demo tick.
struct DemoTickProbe;

impl Render for DemoTickProbe {
    /// The card under the wizard's own text settings, at the window's full width.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .p(design::ui_px(cx, 22.0))
            .font_family(design::ui_font())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .child(
                MoonSelectableCard::new("cw-opt-new")
                    .child(option_body(
                        t!("connect_wizard.start.new_title").to_string(),
                        t!("connect_wizard.start.new_more").to_string(),
                        None,
                        cx,
                    ))
                    .child(demo_tick(MoonCheckbox::new("cw-opt-new-demo"), cx)),
            )
    }
}

/// A click on the tick's line but away from its label is a click on the card: it must pick the
/// card, not turn the demo on. A tick stretched across the card takes every such click.
#[gpui::test]
fn the_demo_tick_takes_no_more_room_than_its_label(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Dark, 1.0),
            app,
        );
    });
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let handle = cx.open_window(
            gpui::size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)),
            |_, _| DemoTickProbe,
        );
        let mut visual = gpui::VisualTestContext::from_window(handle.into(), cx);
        for _ in 0..3 {
            visual.update(|window, app| {
                let _ = window.draw(app);
            });
        }
        let card = visual
            .debug_bounds("moon-selectable-card:cw-opt-new")
            .expect("card");
        let tick = visual.debug_bounds("cw-demo-tick").expect("tick");
        assert!(
            tick.origin.x >= card.origin.x && tick.right() <= card.right(),
            "{language:?}: tick outside the card"
        );
        assert!(
            tick.size.width < card.size.width / 2.0,
            "{language:?}: the tick is {:?} wide in a card of {:?}",
            tick.size.width,
            card.size.width
        );
    }
}

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
            UiThemeMode::Obsidian,
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
