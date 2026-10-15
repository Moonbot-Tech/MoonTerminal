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
use super::{auto_cell_el, auto_cell_text, auto_computing_el, auto_empty_el, auto_progress_row};
use crate::design;

/// Width of the probe window, in font px: a narrow strategy list.
const LIST_W: f32 = 340.0;

/// Every class the column draws, with two-digit counts (the widest a chip gets in practice).
const CLASSES: [AutoClass; 8] = [
    AutoClass::Useful {
        n: 12,
        white: false,
    },
    AutoClass::Useful { n: 12, white: true },
    AutoClass::Noise {
        n: 12,
        white: false,
    },
    AutoClass::Noise { n: 12, white: true },
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
            .child(auto_computing_el(p, cx))
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
                    let computing = visual.debug_bounds("an-auto-computing").expect("computing");
                    inside(computing, list, "computing note");
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

/// The narrowest strategy list the user works with, in font px.
const PROGRESS_LIST_W: f32 = 1000.0;

/// Draws the auto-pick progress line with one sentence, inside a list-width box.
struct ProgressProbe {
    text: String,
    warn: bool,
    fraction: Option<f32>,
}

impl Render for ProgressProbe {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        v_flex()
            .w_full()
            .debug_selector(|| "probe-list".into())
            .child(auto_progress_row(
                self.text.clone(),
                self.warn,
                self.fraction,
                p,
                cx,
            ))
    }
}

/// The auto-pick progress line shows its whole sentence on one line at the list widths the
/// user has, in every language and both zooms. Squeezed back beside the mode buttons and the
/// row counter it rendered as a bare "…", which is what this catches.
#[gpui::test]
fn the_auto_progress_line_is_never_clipped(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Dark, 1.0),
            cx,
        )
    });
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let sentences = [
            (
                t!("analytics.auto.progress", k = 641, m = 641).to_string(),
                false,
                Some(0.5),
            ),
            (
                t!("analytics.auto.done", n = 593, m = 641).to_string(),
                false,
                None,
            ),
            (t!("analytics.auto.busy").to_string(), true, None),
            (t!("analytics.auto.failed").to_string(), true, None),
        ];
        for zoom in [1.0, 1.25] {
            for (text, warn, fraction) in &sentences {
                let probe = ProgressProbe {
                    text: text.clone(),
                    warn: *warn,
                    fraction: *fraction,
                };
                let window = cx.open_window(
                    gpui::size(px(PROGRESS_LIST_W * zoom), px(200.0)),
                    move |_, _| probe,
                );
                let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                visual.update(|window, cx| window.set_content_zoom(zoom, cx));
                for _ in 0..3 {
                    visual.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                }
                let at = format!("{} zoom {zoom} {text:?}", language.code());
                let list = visual.debug_bounds("probe-list").expect("list laid out");
                let row = visual
                    .debug_bounds("an-auto-progress")
                    .unwrap_or_else(|| panic!("progress row not laid out ({at})"));
                let txt = visual
                    .debug_bounds("an-auto-progress-txt")
                    .unwrap_or_else(|| panic!("progress text not laid out ({at})"));
                assert!(
                    row.right() <= list.right() + px(0.5) && txt.right() <= row.right() + px(0.5),
                    "progress line escaped the list ({at})"
                );
                let need = visual.update(|_, cx| design::ui_caption_text_width(cx, text, 400.0));
                assert!(
                    f32::from(txt.size.width) + 0.1 >= need,
                    "progress sentence clipped ({at})"
                );
            }
        }
    }
}
