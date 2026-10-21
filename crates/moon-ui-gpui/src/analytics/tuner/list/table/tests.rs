//! Layout probe for the batch auto-pick's pieces of the strategy list: the "Auto" cell in
//! every class, the toolbar filter in every state and the filter's empty state.
use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    Styled, Window, div, px,
};
use moon_core::config::{Language, UiThemeMode};
use moon_ui::{MoonCheckbox, MoonPalette, MoonTheme, h_flex, v_flex};
use rust_i18n::t;

use super::super::super::STRAT_MODES;
use super::super::super::StratMode;
use super::super::super::auto_picks::{AutoClass, AutoFilter};
use super::super::super::ready::{ReadyClass, ReadyFilter};
use super::super::{auto_filter_el, auto_filter_on_axis, ready_filter_el, ready_filter_on_axis};
use super::{
    auto_cell_el, auto_cell_text, auto_computing_el, auto_empty_el, progress_row, ready_cell_el,
    ready_cell_text, ready_computing_el, ready_empty_el, strat_list_frame,
};
use crate::design;

/// The strategy-list header and readiness progress at the card's actual available width.
struct HeaderLayoutProbe {
    text: String,
}

impl Render for HeaderLayoutProbe {
    /// Draw the header controls and a single-line ETA caption inside the available width.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        v_flex()
            .w_full()
            .debug_selector(|| "header-list".into())
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .items_center()
                    .gap(design::ui_px(cx, 6.0))
                    .child(
                        div()
                            .debug_selector(|| "header-search".into())
                            .flex_1()
                            .min_w(design::ui_px(cx, 120.0))
                            .h(design::ui_px(cx, 22.0)),
                    )
                    .child(
                        div()
                            .debug_selector(|| "header-type".into())
                            .flex_none()
                            .w(design::ui_px(cx, 116.0))
                            .h(design::ui_px(cx, 22.0)),
                    )
                    .child(
                        div()
                            .debug_selector(|| "header-lists".into())
                            .flex_none()
                            .w(design::ui_px(cx, 96.0))
                            .h(design::ui_px(cx, 22.0)),
                    )
                    .child(ready_filter_el(ReadyFilter::Can, Rc::new(|_, _| {}), cx))
                    .child(
                        div().debug_selector(|| "header-active".into()).child(
                            MoonCheckbox::new("header-active-box")
                                .label(t!("analytics.strat.active_only").to_string()),
                        ),
                    )
                    .child(
                        div()
                            .debug_selector(|| "header-cols".into())
                            .flex_none()
                            .w(design::ui_px(cx, 30.0))
                            .h(design::ui_px(cx, 22.0)),
                    ),
            )
            .child(super::progress_row(
                self.text.clone(),
                false,
                Some(0.5),
                true,
                p,
                cx,
            ))
    }
}

/// A long ETA remains one line while filter controls wrap inside the card.
///
/// Dropping the pick-best button and its top-N menu must not be what makes this fit: the
/// remaining controls and the readiness line still have to stay inside the card.
#[gpui::test]
fn header_and_eta_fit_every_language_theme_zoom_and_list_width(cx: &mut gpui::TestAppContext) {
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
                for width in [LIST_W, CARD_WIDTHS[0], CARD_WIDTHS[1], CARD_WIDTHS[2]] {
                    let text = t!(
                        "analytics.ready.progress_eta",
                        k = 123,
                        m = 641,
                        eta = super::super::super::ticks::estimate::duration_text(
                            std::time::Duration::from_secs(7200)
                        )
                    )
                    .to_string();
                    let window = cx.open_window(gpui::size(px(width * zoom), px(400.0)), |_, _| {
                        HeaderLayoutProbe { text }
                    });
                    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                    visual.update(|window, cx| window.set_content_zoom(zoom, cx));
                    for _ in 0..3 {
                        visual.update(|window, cx| {
                            let _ = window.draw(cx);
                        });
                    }
                    let list = visual.debug_bounds("header-list").expect("list");
                    for sel in [
                        "header-search",
                        "header-type",
                        "header-lists",
                        "header-active",
                        "header-cols",
                        "an-strat-ready-tip",
                        "an-ready-progress-txt",
                    ] {
                        let b = visual.debug_bounds(sel).expect(sel);
                        assert!(
                            b.left() >= list.left() - px(0.5)
                                && b.right() <= list.right() + px(0.5),
                            "{sel} escaped: {} {mode:?} {zoom} {width}",
                            language.code()
                        );
                    }
                    let caption = visual
                        .debug_bounds("an-ready-progress-txt")
                        .expect("caption");
                    assert!(
                        caption.size.height <= px(24.0 * zoom),
                        "ETA wrapped: {} {mode:?} {zoom} {width}",
                        language.code()
                    );
                }
            }
        }
    }
}

/// Render the exact readiness tooltip factory instead of a hand-built substitute for its text.
struct ReadyTipProbe;

impl Render for ReadyTipProbe {
    /// Use full exclusion counts and an actionable fetchable line to exercise the longest tip.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = super::super::super::ready::cell_tip(
            &super::super::super::ticks::readiness::Readiness {
                deals: 999,
                no_tape: 100,
                short_tail: 100,
                holed: 100,
                missed: 100,
                unjudged: 100,
                fit: 499,
                covered: 899,
                fetchable: 100,
            },
        );
        let tip = crate::panels::common::text_tooltip(text)(window, cx);
        v_flex().w_full().child(
            div()
                .debug_selector(|| "ready-tip".into())
                .flex()
                .self_start()
                .child(tip),
        )
    }
}

/// Losing MoonTooltip's width cap or wrapping would overflow the readiness pill's breakdown.
#[gpui::test]
fn readiness_breakdown_tooltip_wraps_inside_its_max_width(cx: &mut gpui::TestAppContext) {
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
                let window = cx.open_window(gpui::size(px(LIST_W * zoom), px(900.0)), |_, _| {
                    ReadyTipProbe
                });
                let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                visual.update(|window, cx| window.set_content_zoom(zoom, cx));
                for _ in 0..3 {
                    visual.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                }
                let tip = visual.debug_bounds("ready-tip").expect("tooltip");
                assert!(
                    tip.size.width <= px(260.0 * zoom + 0.5),
                    "tooltip exceeds max width: {} {mode:?} {zoom}",
                    language.code()
                );
                // Four explicit lines plus a wrapped breakdown must occupy more than four caption lines.
                assert!(
                    tip.size.height > px(4.0 * 14.0 * zoom),
                    "tooltip failed to wrap: {} {mode:?} {zoom}",
                    language.code()
                );
            }
        }
    }
}

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
            .child(progress_row(
                self.text.clone(),
                self.warn,
                self.fraction,
                false,
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
    mode: StratMode,
    text: String,
    warn: bool,
    fraction: Option<f32>,
}

impl Render for CardProgressProbe {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let progress = (self.mode.shows_auto() || self.mode.shows_ready()).then(|| {
            progress_row(
                self.text.clone(),
                self.warn,
                self.fraction,
                self.mode.shows_ready(),
                p,
                cx,
            )
        });
        let toolbar = super::strat_toolbar(
            |id, mode, label| {
                moon_ui::MoonButton::new(id)
                    .label(label)
                    .selected(mode == self.mode)
                    .render()
                    .into_any_element()
            },
            progress,
            Some((
                t!("analytics.strat.shown_compact", shown = 9, total = 140).to_string(),
                t!("analytics.strat.shown", shown = 9, total = 140).to_string(),
                p.text_muted,
            )),
            p,
            cx,
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
        let card = strat_list_frame(p, toolbar, filter, header, table_row, cx);
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
                    mode: StratMode::Coins,
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
                let filter = visual.debug_bounds("an-strat-filter").expect("filter");
                assert!(
                    (filter.top() - toolbar.bottom()).abs() <= px(0.5),
                    "filter must immediately follow toolbar ({at})"
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
                    row.top() >= toolbar.top() && row.bottom() <= toolbar.bottom(),
                    "progress line must sit inside the toolbar ({at})"
                );
                assert!(
                    table.origin.y + px(0.5) >= row.bottom(),
                    "first strategy row must sit below the progress line ({at})"
                );
            }
        }
    }
}

/// Switching modes must never insert a status strip or push the filter bar down.
#[gpui::test]
fn toolbar_and_filter_share_an_edge_in_every_mode(cx: &mut gpui::TestAppContext) {
    for theme in [
        UiThemeMode::Light,
        UiThemeMode::Dark,
        UiThemeMode::Graphite,
        UiThemeMode::LightExperimental,
        UiThemeMode::DarkExperimental,
    ] {
        cx.update(|cx| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(theme, 1.0),
                cx,
            )
        });
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            for width in [740.0, 900.0, 1330.0] {
                let mut baseline = None;
                for mode in STRAT_MODES {
                    let probe = CardProgressProbe {
                        mode,
                        text: t!("analytics.ready.progress_eta", k = 5, m = 595, eta = "2h")
                            .to_string(),
                        warn: false,
                        fraction: Some(0.4),
                    };
                    let window =
                        cx.open_window(gpui::size(px(width + 20.0), px(560.0)), |_, _| probe);
                    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                    for _ in 0..3 {
                        visual.update(|window, cx| {
                            let _ = window.draw(cx);
                        });
                    }
                    let toolbar = visual.debug_bounds("an-strat-toolbar").expect("toolbar");
                    let filter = visual.debug_bounds("an-strat-filter").expect("filter");
                    assert!((filter.top() - toolbar.bottom()).abs() <= px(0.5));
                    if let Some(y) = baseline {
                        assert!(
                            (filter.top() - y).abs() <= px(0.5),
                            "mode switch moved the filter bar"
                        );
                    }
                    baseline = Some(filter.top());
                    let counter = visual.debug_bounds("an-strat-counter").expect("counter");
                    assert!(
                        counter.right() <= toolbar.right() + px(0.5),
                        "counter overflow {} {width}",
                        language.code()
                    );
                    if mode.shows_auto() || mode.shows_ready() {
                        let selector = if mode.shows_auto() {
                            "an-auto-progress-txt"
                        } else {
                            "an-ready-progress-txt"
                        };
                        let text = visual.debug_bounds(selector).expect("caption");
                        assert!(
                            text.right() <= counter.left()
                                && text.top() >= toolbar.top()
                                && text.bottom() <= toolbar.bottom(),
                            "caption overflow {} {width}",
                            language.code()
                        );
                    }
                }
            }
        }
    }
}

/// Width of the toolbar probe, in font px: the narrowest strategy card ([`CARD_WIDTHS`]).
const TOOLBAR_W: f32 = CARD_WIDTHS[0];

/// Stretching a short caption leaves its glyphs and progress bar stranded beside the mode tabs.
#[gpui::test]
fn short_progress_caption_sits_against_the_counter(cx: &mut gpui::TestAppContext) {
    let _locale = crate::test_locale::force("en");
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0),
            cx,
        )
    });
    for mode in [StratMode::Coins, StratMode::Ticks] {
        let window = cx.open_window(gpui::size(px(1350.0), px(560.0)), |_, _| {
            CardProgressProbe {
                mode,
                text: "12/140".into(),
                warn: false,
                fraction: Some(0.4),
            }
        });
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        for _ in 0..3 {
            visual.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        let selector = if mode.shows_auto() {
            "an-auto-progress-txt"
        } else {
            "an-ready-progress-txt"
        };
        let caption = visual.debug_bounds(selector).expect("caption");
        let counter = visual.debug_bounds("an-strat-counter").expect("counter");
        // GPUI's TextLayout rounds intrinsic widths up to whole layout pixels.
        let needed =
            visual.update(|_, cx| design::ui_caption_text_width(cx, "12/140", 400.0).ceil());
        assert!(
            f32::from(caption.size.width) <= needed + 0.5,
            "the caption must retain its intrinsic width on a wide card: {:?}, need {needed}",
            caption.size.width
        );
        assert!(
            (counter.left() - caption.right() - px(8.0)).abs() <= px(0.5),
            "short progress must hug the counter across the toolbar gap"
        );
    }
}

/// Omitting the condition marker from sizing hides it when a short numeric column is sorted.
#[gpui::test]
fn numeric_headers_fit_filter_markers_and_sort_arrows(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0),
            cx,
        )
    });
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        cx.update(|cx| {
            let widths = super::metric_col_widths(&[], cx);
            for (column, width) in super::METRIC_COLS.iter().zip(widths) {
                for arrow in ["▲", "▼"] {
                    let heading = format!("{} · {arrow}", t!(column.key));
                    let needed = design::mono_body_text_width(cx, &heading, 400.0);
                    assert!(
                        needed <= width,
                        "filtered sorted header clipped: {} {}",
                        language.code(),
                        column.key
                    );
                }
            }
        });
    }
}

/// The strategy list's filter bar as `strat_filter_bar` lays it out on one axis: the search
/// box takes the rest, then the type and coin-list pickers, the auto-pick filter on "By coin"
/// only, the enabled-only toggle and the column selector.
struct ToolbarProbe {
    mode: StratMode,
    filter: AutoFilter,
}

impl Render for ToolbarProbe {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let fixed = |id: &'static str, w: f32, cx: &App| {
            div()
                .debug_selector(move || id.into())
                .flex_none()
                .w(design::ui_px(cx, w))
                .h(design::ui_px(cx, 22.0))
        };
        h_flex()
            .debug_selector(|| "probe-bar".into())
            .w_full()
            .px(design::ui_px(cx, 8.0))
            .gap(design::ui_px(cx, 6.0))
            .items_center()
            .child(
                div()
                    .debug_selector(|| "probe-search".into())
                    .flex_1()
                    .min_w_0()
                    .h(design::ui_px(cx, 22.0)),
            )
            .child(fixed("probe-type", 116.0, cx))
            .child(fixed("probe-lists", 96.0, cx))
            .children(auto_filter_on_axis(
                self.mode,
                self.filter,
                Rc::new(|_, _| {}),
                cx,
            ))
            .children(ready_filter_on_axis(
                self.mode,
                ReadyFilter::Can,
                Rc::new(|_, _| {}),
                cx,
            ))
            .child(
                div().debug_selector(|| "probe-active".into()).child(
                    MoonCheckbox::new("probe-active-box")
                        .label(t!("analytics.strat.active_only").to_string()),
                ),
            )
            .child(fixed("probe-cols", 30.0, cx))
    }
}

/// The auto-pick filter is in the strategy filter bar on "By coin" only; on the three other
/// axes it is absent. Either way every control sits inside the bar, left to right, without
/// overlapping its neighbour, in every language and both zooms.
#[gpui::test]
fn the_auto_filter_is_in_the_bar_on_by_coin_only(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        MoonTheme::install_config(
            crate::startup::moon_theme_config_for_presentation(UiThemeMode::Dark, 1.0),
            cx,
        )
    });
    for language in Language::ALL {
        let _locale = crate::test_locale::force(language.code());
        for zoom in [1.0, 1.25] {
            for mode in STRAT_MODES {
                let filter = AutoFilter::Confirmed;
                let window = cx.open_window(gpui::size(px(TOOLBAR_W * zoom), px(120.0)), |_, _| {
                    ToolbarProbe { mode, filter }
                });
                let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
                visual.update(|window, cx| window.set_content_zoom(zoom, cx));
                for _ in 0..3 {
                    visual.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                }
                let at = format!("{} zoom {zoom} {}", language.code(), mode.shows_auto());
                let bar = visual.debug_bounds("probe-bar").expect("bar laid out");
                let auto = visual.debug_bounds("an-strat-auto-tip");
                assert_eq!(
                    auto.is_some(),
                    mode == StratMode::Coins,
                    "auto filter presence ({at})"
                );
                let ready = visual.debug_bounds("an-strat-ready-tip");
                assert_eq!(
                    ready.is_some(),
                    mode == StratMode::Ticks,
                    "tuning filter presence ({at})"
                );
                let mut row = vec!["probe-search", "probe-type", "probe-lists"];
                if auto.is_some() {
                    row.push("an-strat-auto-tip");
                }
                if ready.is_some() {
                    row.push("an-strat-ready-tip");
                }
                row.extend(["probe-active", "probe-cols"]);
                let mut prev: Option<gpui::Bounds<gpui::Pixels>> = None;
                for sel in row {
                    let b = visual
                        .debug_bounds(sel)
                        .unwrap_or_else(|| panic!("{sel} not laid out ({at})"));
                    assert!(
                        b.origin.x + px(0.5) >= bar.origin.x && b.right() <= bar.right() + px(0.5),
                        "{sel} escaped the bar ({at})"
                    );
                    if let Some(p) = prev {
                        assert!(b.origin.x + px(0.5) >= p.right(), "{sel} overlaps ({at})");
                    }
                    prev = Some(b);
                }
                if let Some(trigger) = ready {
                    let label = t!(
                        "analytics.ready.filter_lbl",
                        value = ReadyFilter::Can.label()
                    )
                    .to_string();
                    let need = visual.update(|_, cx| design::ui_body_text_width(cx, &label, 400.0));
                    assert!(
                        f32::from(trigger.size.width) >= need,
                        "tuning filter narrower than {label:?} ({at})"
                    );
                }
                if let Some(trigger) = auto {
                    let label = t!("analytics.auto.filter_lbl", value = filter.label()).to_string();
                    let need = visual.update(|_, cx| design::ui_body_text_width(cx, &label, 400.0));
                    assert!(
                        f32::from(trigger.size.width) >= need,
                        "auto filter narrower than {label:?} ({at})"
                    );
                }
            }
        }
    }
}

/// Every class the "Tuning" column draws, with two-digit counts.
const READY_CLASSES: [ReadyClass; 5] = [
    ReadyClass::Can { fit: 34 },
    ReadyClass::Few { fit: 10 },
    ReadyClass::NoTape,
    ReadyClass::Pending,
    ReadyClass::Unread,
];

/// Draws one "Tuning" cell per class, the filter in one state, and both empty faces.
struct ReadyLayoutProbe {
    filter: ReadyFilter,
}

impl Render for ReadyLayoutProbe {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let cells: Vec<AnyElement> = READY_CLASSES
            .iter()
            .enumerate()
            .map(|(i, class)| {
                h_flex()
                    .w_full()
                    .child(div().flex_1())
                    .child(ready_cell_el(
                        SharedString::from(format!("probe-ready-{i}")),
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
            .child(h_flex().child(ready_filter_el(self.filter, Rc::new(|_, _| {}), cx)))
            .child(ready_empty_el(|_, _, _| {}, p, cx))
            .child(ready_computing_el(p, cx))
    }
}

/// Every "Tuning" cell is drawn whole inside its column, the filter's trigger is never
/// narrower than its label, and the empty faces stay inside the list, in both themes, every
/// language and both zooms.
#[gpui::test]
fn the_ready_column_filter_and_empty_state_fit_every_language(cx: &mut gpui::TestAppContext) {
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
                for filter in ReadyFilter::ALL {
                    let window = cx
                        .open_window(gpui::size(px(LIST_W * zoom), px(900.0)), |_, _| {
                            ReadyLayoutProbe { filter }
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
                    for (i, class) in READY_CLASSES.iter().enumerate() {
                        let cell = visual
                            .debug_bounds(leak(format!("probe-ready-{i}-cell")))
                            .unwrap_or_else(|| panic!("cell {i} not laid out ({at})"));
                        inside(cell, list, "ready cell");
                        let Some((text, _)) = ready_cell_text(*class) else {
                            continue;
                        };
                        let txt = visual
                            .debug_bounds(leak(format!("probe-ready-{i}-txt")))
                            .unwrap_or_else(|| panic!("cell {i} text not laid out ({at})"));
                        inside(txt, cell, &format!("ready cell text {text:?}"));
                        let need =
                            visual.update(|_, cx| design::ui_caption_text_width(cx, &text, 400.0));
                        assert!(
                            f32::from(txt.size.width) + 0.1 >= need,
                            "ready cell text {text:?} narrower than itself ({at})"
                        );
                    }
                    let trigger = visual.debug_bounds("an-strat-ready-tip").expect("filter");
                    inside(trigger, list, "ready filter");
                    let label =
                        t!("analytics.ready.filter_lbl", value = filter.label()).to_string();
                    let need = visual.update(|_, cx| design::ui_body_text_width(cx, &label, 400.0));
                    assert!(
                        f32::from(trigger.size.width) >= need,
                        "ready filter trigger narrower than {label:?} ({at})"
                    );
                    let computing = visual
                        .debug_bounds("an-ready-computing")
                        .expect("computing");
                    inside(computing, list, "computing note");
                    let empty = visual.debug_bounds("an-ready-empty").expect("empty state");
                    inside(empty, list, "empty state");
                    for sel in [
                        "an-ready-empty-title",
                        "an-ready-empty-body",
                        "an-ready-empty-reset",
                    ] {
                        inside(visual.debug_bounds(sel).expect(sel), empty, sel);
                    }
                }
            }
        }
    }
}
