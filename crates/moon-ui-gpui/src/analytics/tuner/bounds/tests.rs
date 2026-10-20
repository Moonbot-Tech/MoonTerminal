//! The shipped bound header is measured against the input slots at normal and narrow widths.

use super::{GridLayout, VariantHead, header};
use crate::design;
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, Window, div, px,
};
use moon_core::config::{Language, UiThemeMode};
use moon_ui::{MoonCheckbox, MoonPalette, MoonTheme, v_flex};

/// The same production header and geometry that both fitting modes render.
struct Probe {
    time: bool,
}

impl Render for Probe {
    /// Place synthetic input slots under each header, including narrow variant sections.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = GridLayout::new(window.viewport_size().width, self.time, cx);
        let mut content = v_flex().w_full();
        for variants in layout.sections() {
            let heads = variants
                .iter()
                .map(|&index| VariantHead {
                    index,
                    copy: div()
                        .w(design::ui_px(cx, 12.0))
                        .flex_none()
                        .child(if index == 0 { "→" } else { "←" })
                        .into_any_element(),
                    clear: div()
                        .w(design::ui_px(cx, 12.0))
                        .flex_none()
                        .child("✕")
                        .into_any_element(),
                })
                .collect();
            let check = || {
                div()
                    .flex_none()
                    .child(MoonCheckbox::new("probe-check").size(design::CONTROL_TIER))
                    .into_any_element()
            };
            content = content.child(header(
                layout,
                format!("probe-head-{}", variants[0]),
                check(),
                layout.name_cell().child("Field").into_any_element(),
                heads,
                MoonPalette::active(cx),
                cx,
            ));
            let mut row = layout
                .row(cx)
                .child(check())
                .child(layout.name_cell().child("WorkingTime"));
            if !layout.compact {
                row = row.child(div().flex_1().min_w_0());
            }
            for &index in &variants {
                for side in ["from", "to"] {
                    if side == "to" {
                        row = row.child(div().w(design::ui_px(cx, 12.0)).flex_none());
                    }
                    row = row.child(
                        div()
                            .debug_selector(move || format!("bound-input-{side}-{index}"))
                            .w(design::font_w_px(cx, layout.input))
                            .flex_none()
                            .child("1.22:45"),
                    );
                }
                row = row.child(div().w(design::ui_px(cx, 12.0)).flex_none());
            }
            content = content.child(row);
        }
        content
    }
}

/// A wrapped header, misplaced copy arrow, or different row width breaks alignment in any locale.
#[gpui::test]
fn livefix_bound_headers_are_one_row_and_align_with_inputs(cx: &mut gpui::TestAppContext) {
    for theme in [UiThemeMode::Light, UiThemeMode::Dark, UiThemeMode::Graphite] {
        cx.update(|cx| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(theme, 1.0),
                cx,
            )
        });
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            for time in [false, true] {
                for width in [300.0, 380.0, 450.0, 680.0, 900.0] {
                    let window =
                        cx.open_window(gpui::size(px(width), px(900.0)), |_, _| Probe { time });
                    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                    for _ in 0..3 {
                        visual.update(|window, cx| {
                            let _ = window.draw(cx);
                        });
                    }
                    let compact = visual.update(|window, cx| {
                        GridLayout::new(window.viewport_size().width, time, cx).compact
                    });
                    for index in 0..2 {
                        let mut top = None;
                        for side in ["from", "to"] {
                            let head = visual
                                .debug_bounds(Box::leak(
                                    format!("bound-head-{side}-{index}").into_boxed_str(),
                                ))
                                .unwrap();
                            let input = visual
                                .debug_bounds(Box::leak(
                                    format!("bound-input-{side}-{index}").into_boxed_str(),
                                ))
                                .unwrap();
                            assert!((head.left() - input.left()).abs() < px(0.5));
                            assert!((head.right() - input.right()).abs() < px(0.5));
                            assert!(
                                head.right() <= px(width),
                                "header escaped in {} at {width}",
                                language.code()
                            );
                            let section = visual
                                .debug_bounds(if !compact || index == 0 {
                                    "probe-head-0"
                                } else {
                                    "probe-head-1"
                                })
                                .unwrap();
                            assert!(
                                section.size.height <= head.size.height + px(8.0),
                                "bound header occupies more than one row"
                            );
                            if let Some(top) = top {
                                assert_eq!(head.top(), top, "header wrapped");
                            }
                            top = Some(head.top());
                        }
                    }
                    if !compact {
                        let current = visual
                            .debug_bounds("bound-head-current")
                            .expect("full-width current heading");
                        let need = visual.update(|_, cx| {
                            design::ui_body_text_width(
                                cx,
                                &rust_i18n::t!("analytics.time.cur"),
                                400.0,
                            )
                        });
                        assert!(
                            current.size.width + px(0.5) >= px(need),
                            "current heading clipped in {}",
                            language.code()
                        );
                    }
                }
            }
        }
    }
}
