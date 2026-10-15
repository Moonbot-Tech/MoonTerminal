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
use super::{
    auto_cell_el, auto_cell_text, auto_computing_el, auto_empty_el, auto_progress_row,
    strat_list_frame,
};
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

/// Card widths the strategy list is used at, in font px. 1330 is the width where the live
/// card wrapped the progress line one glyph per line.
const CARD_WIDTHS: [f32; 3] = [900.0, 1330.0, 1800.0];

/// The strategy card in the same column the strategies tab uses: the list shares the column
/// with a detail pane, and the column is a flex child. A window-sized box around the progress
/// line alone is what let the previous probe pass.
struct CardProgressProbe {
    text: String,
    warn: bool,
    fraction: Option<f32>,
}

impl Render for CardProgressProbe {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let toolbar = h_flex()
            .debug_selector(|| "an-strat-toolbar".into())
            .w_full()
            .flex_none()
            .px(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 8.0))
            .items_center()
            .gap(design::ui_px(cx, 8.0))
            .font_family(design::ui_font())
            .child(div().flex_none().child("list"))
            .child(div().flex_none().child("mode"))
            .child(div().flex_1())
            .child(
                div()
                    .flex_none()
                    .text_size(design::t_caption(cx))
                    .child("shown"),
            );
        let filter = div()
            .debug_selector(|| "an-strat-filter".into())
            .w_full()
            .flex_none()
            .h(design::ui_px(cx, 32.0));
        let header = div()
            .debug_selector(|| "an-strat-header".into())
            .w_full()
            .flex_none()
            .h(design::ui_px(cx, 28.0));
        let table_row = div()
            .debug_selector(|| "an-strat-probe-row".into())
            .w_full()
            .flex_none()
            .h(design::ui_px(cx, 28.0))
            .child("row");
        let card = strat_list_frame(
            p,
            toolbar,
            Some(auto_progress_row(
                self.text.clone(),
                self.warn,
                self.fraction,
                p,
                cx,
            )),
            filter,
            header,
            table_row,
            cx,
        );
        div().size_full().child(
            h_flex()
                .size_full()
                .p(design::ui_px(cx, 10.0))
                .gap(design::ui_px(cx, 8.0))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .min_h_0()
                        .gap(design::ui_px(cx, 8.0))
                        .child(card)
                        .child(div().flex_none().w_full().h(design::ui_px(cx, 160.0))),
                )
                .child(
                    // Out of flow, so it does not steal width from the card. One caption line,
                    // the same face the progress row uses, is the height the row must match.
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .debug_selector(|| "probe-line".into())
                        .flex_none()
                        .whitespace_nowrap()
                        .font_family(design::ui_font())
                        .text_size(design::t_caption(cx))
                        .child("Ag"),
                ),
        )
    }
}

/// The auto-pick line is one caption line tall and as wide as the strategy card, on every
/// tab that shows the list. The toolbar stays above it and the first strategy row stays
/// inside the card below it.
///
/// A text child with `min_w_0` and no nowrap, inside this card, wraps one glyph per line.
/// At 1330 px that strip is hundreds of pixels tall and pushes the table out of the card.
/// The standalone probe at 1000 px never built this column, so it stayed green.
#[gpui::test]
fn the_auto_progress_line_is_one_row_of_the_strategy_card(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Dark, 1.0),
            cx,
        )
    });
    let (pad, window_h) = cx.update(|cx| {
        (
            f32::from(design::ui_px(cx, 10.0)),
            f32::from(design::ui_px(cx, 560.0)),
        )
    });
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        let sentences = [
            (
                t!("analytics.auto.progress", k = 5, m = 595).to_string(),
                false,
                Some(0.4),
            ),
            (
                t!("analytics.auto.done", n = 5, m = 595).to_string(),
                false,
                None,
            ),
            (t!("analytics.auto.busy").to_string(), true, None),
            (t!("analytics.auto.failed").to_string(), true, None),
        ];
        for card_w in CARD_WIDTHS {
            for (text, warn, fraction) in &sentences {
                let probe = CardProgressProbe {
                    text: text.clone(),
                    warn: *warn,
                    fraction: *fraction,
                };
                let window = cx.open_window(
                    gpui::size(px(card_w + pad * 2.0), px(window_h)),
                    move |_, _| probe,
                );
                let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                for _ in 0..3 {
                    visual.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                }
                let at = format!("{} card {card_w} {text:?}", language.code());
                let card = visual.debug_bounds("an-strat-card").expect("card");
                let row = visual
                    .debug_bounds("an-auto-progress")
                    .unwrap_or_else(|| panic!("progress row not laid out ({at})"));
                let txt = visual
                    .debug_bounds("an-auto-progress-txt")
                    .unwrap_or_else(|| panic!("progress text not laid out ({at})"));
                let toolbar = visual.debug_bounds("an-strat-toolbar").expect("toolbar");
                let table = visual
                    .debug_bounds("an-strat-probe-row")
                    .expect("first row");
                let line = visual.debug_bounds("probe-line").expect("caption line");
                let h = |b: gpui::Bounds<gpui::Pixels>| f32::from(b.size.height);
                let w = |b: gpui::Bounds<gpui::Pixels>| f32::from(b.size.width);
                assert!(
                    (w(card) - card_w).abs() <= 2.0,
                    "card width {} is not {card_w} ({at})",
                    w(card)
                );
                assert!(
                    (h(txt) - h(line)).abs() <= 1.0,
                    "progress text is not one line: text {} line {} row {} ({at})",
                    h(txt),
                    h(line),
                    h(row)
                );
                assert!(
                    (h(row) - h(line)).abs() <= 1.0,
                    "progress row is not one line: row {} line {} ({at})",
                    h(row),
                    h(line)
                );
                assert!(
                    w(row) + 1.0 >= w(card) - 2.0,
                    "progress row width {} does not span card {} ({at})",
                    w(row),
                    w(card)
                );
                let inside = |b: gpui::Bounds<gpui::Pixels>, what: &str| {
                    assert!(
                        b.origin.y + px(0.5) >= card.origin.y
                            && b.bottom() <= card.bottom() + px(0.5)
                            && b.origin.x + px(0.5) >= card.origin.x
                            && b.right() <= card.right() + px(0.5)
                            && h(b) > 1.0,
                        "{what} is not visible inside the card ({at}) card={card:?} item={b:?}"
                    );
                };
                inside(toolbar, "toolbar");
                inside(table, "first strategy row");
                assert!(
                    toolbar.bottom() <= row.origin.y + px(0.5),
                    "progress line must sit under the toolbar ({at})"
                );
                assert!(
                    table.origin.y + px(0.5) >= row.bottom(),
                    "first strategy row must sit below the progress line ({at})"
                );
            }
        }
    }
}
