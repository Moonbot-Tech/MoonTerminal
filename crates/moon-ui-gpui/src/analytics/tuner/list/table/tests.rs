//! Layout probe for the batch auto-pick's pieces of the strategy list: the "Auto" cell in
//! every class, the toolbar filter in every state and the filter's empty state.
use std::rc::Rc;

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    Styled, Window, div, px,
};
use moon_core::config::{Language, UiThemeMode};
use moon_ui::{MoonPalette, MoonTheme, h_flex, v_flex};
use rust_i18n::t;

use super::super::super::auto_picks::{AutoClass, AutoFilter};
use super::super::auto_filter_el;
use super::{auto_cell_el, auto_cell_text, auto_empty_el};
use crate::design;

/// Width of the probe window, in font px: a narrow strategy list.
const LIST_W: f32 = 340.0;

/// Every class the column draws, with two-digit counts (the widest a chip gets in practice).
const CLASSES: [AutoClass; 7] = [
    AutoClass::Useful {
        n: 12,
        white: false,
    },
    AutoClass::Useful { n: 12, white: true },
    AutoClass::Noise { n: 12 },
    AutoClass::Nothing,
    AutoClass::Few,
    AutoClass::Pending,
    AutoClass::Unread,
];

/// Draws one cell per class, the filter in one state, and the empty state.
struct AutoLayoutProbe {
    filter: AutoFilter,
}

impl Render for AutoLayoutProbe {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let cells: Vec<AnyElement> = CLASSES
            .iter()
            .enumerate()
            .map(|(i, class)| {
                h_flex()
                    .w_full()
                    .child(div().flex_1())
                    .child(auto_cell_el(
                        SharedString::from(format!("probe-auto-{i}")),
                        *class,
                        None,
                        p,
                        cx,
                    ))
                    .into_any_element()
            })
            .collect();
        v_flex()
            .w_full()
            .debug_selector(|| "probe-list".into())
            .children(cells)
            .child(h_flex().child(auto_filter_el(self.filter, Rc::new(|_, _| {}), cx)))
            .child(auto_empty_el(5, 30, |_, _, _| {}, p, cx))
    }
}

/// `debug_bounds` takes a `'static` selector; the probe's selectors are built per run.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Every chip and mark of the Auto column is drawn whole inside its cell, the filter's trigger
/// is never narrower than its label, and the empty state stays inside the list, in both themes,
/// every language and both zooms. A longer translation, a wider count or a narrower column
/// breaks it.
#[gpui::test]
fn the_auto_column_filter_and_empty_state_fit_every_language(cx: &mut gpui::TestAppContext) {
    for mode in [UiThemeMode::Light, UiThemeMode::Dark] {
        cx.update(|cx| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(mode, 1.0),
                cx,
            )
        });
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            for zoom in [1.0, 1.25] {
                for filter in AutoFilter::ALL {
                    let window = cx
                        .open_window(gpui::size(px(LIST_W * zoom), px(900.0)), |_, _| {
                            AutoLayoutProbe { filter }
                        });
                    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                    visual.update(|window, cx| window.set_content_zoom(zoom, cx));
                    for _ in 0..3 {
                        visual.update(|window, cx| {
                            let _ = window.draw(cx);
                        });
                    }
                    let at = format!(
                        "{} {mode:?} zoom {zoom} filter {}",
                        language.code(),
                        filter.key()
                    );
                    let list = visual.debug_bounds("probe-list").expect("list laid out");
                    let inside = |b: gpui::Bounds<gpui::Pixels>,
                                  outer: gpui::Bounds<gpui::Pixels>,
                                  what: &str| {
                        assert!(
                            b.origin.x + px(0.5) >= outer.origin.x
                                && b.right() <= outer.right() + px(0.5),
                            "{what} escaped its box ({at})"
                        );
                    };
                    for (i, class) in CLASSES.iter().enumerate() {
                        let cell = visual
                            .debug_bounds(leak(format!("probe-auto-{i}-cell")))
                            .unwrap_or_else(|| panic!("cell {i} not laid out ({at})"));
                        inside(cell, list, "auto cell");
                        let Some((text, _)) = auto_cell_text(*class) else {
                            continue;
                        };
                        let txt = visual
                            .debug_bounds(leak(format!("probe-auto-{i}-txt")))
                            .unwrap_or_else(|| panic!("cell {i} text not laid out ({at})"));
                        inside(txt, cell, &format!("auto cell text {text:?}"));
                        let need =
                            visual.update(|_, cx| design::ui_caption_text_width(cx, &text, 400.0));
                        assert!(
                            f32::from(txt.size.width) + 0.1 >= need,
                            "auto cell text {text:?} narrower than itself ({at})"
                        );
                    }
                    let trigger = visual.debug_bounds("an-strat-auto-tip").expect("filter");
                    inside(trigger, list, "auto filter");
                    let label = t!("analytics.auto.filter_lbl", value = filter.label()).to_string();
                    let need = visual.update(|_, cx| design::ui_body_text_width(cx, &label, 400.0));
                    assert!(
                        f32::from(trigger.size.width) >= need,
                        "auto filter trigger narrower than {label:?} ({at})"
                    );
                    let empty = visual.debug_bounds("an-auto-empty").expect("empty state");
                    inside(empty, list, "empty state");
                    for sel in [
                        "an-auto-empty-title",
                        "an-auto-empty-body",
                        "an-auto-empty-reset",
                    ] {
                        inside(visual.debug_bounds(sel).expect(sel), empty, sel);
                    }
                    let reset = visual.debug_bounds("an-auto-empty-reset").expect("reset");
                    let label = t!("analytics.auto.show_all").to_string();
                    let need = visual.update(|_, cx| design::ui_body_text_width(cx, &label, 400.0));
                    assert!(
                        f32::from(reset.size.width) >= need,
                        "reset button narrower than {label:?} ({at})"
                    );
                }
            }
        }
    }
}
