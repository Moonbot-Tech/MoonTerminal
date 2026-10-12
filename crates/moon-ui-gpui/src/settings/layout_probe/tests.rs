//! Self-check of the layout probe, and the first sweeps over the shared page chrome.

use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext, Context, InteractiveElement, IntoElement, ParentElement, Render,
    ScrollHandle, StatefulInteractiveElement, Styled, Window, div, px,
};
use moon_ui::MoonInputState;

use super::{ProbeSpec, Probed, TextFace, probe_once, sweep};
use crate::settings::Tab;
use crate::settings::common::{hint_row, num_input};
use crate::settings::frame::{PageHeader, SaveBar, page_header, save_bar};

/// A probe host drawing whatever `build` returns inside the body box.
struct Host {
    width: f32,
    height: f32,
    scroll: ScrollHandle,
    build: Rc<dyn Fn(&mut Window, &mut App, f32) -> AnyElement>,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = (self.build)(window, cx, self.width);
        div()
            .w(px(self.width))
            .h(px(self.height))
            .debug_selector(|| "probe-body".into())
            .child(
                div()
                    .id("probe-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(div().w_full().flex_none().child(content)),
            )
    }
}

impl Probed for Host {
    fn scroll(&self) -> Option<ScrollHandle> {
        Some(self.scroll.clone())
    }
}

fn host(
    build: impl Fn(&mut Window, &mut App, f32) -> AnyElement + 'static,
) -> impl Fn(&mut Window, &mut Context<Host>, (f32, f32)) -> Host {
    let build: Rc<dyn Fn(&mut Window, &mut App, f32) -> AnyElement> = Rc::new(build);
    move |_, _, (width, height)| Host {
        width,
        height,
        scroll: ScrollHandle::new(),
        build: build.clone(),
    }
}

const LONG: &str = "A deliberately long help text that has to wrap onto several lines in any \
     settings column the probe can produce, so a fixed one-line height would cut it off.";

/// Catches a probe that passes everything: two boxes drawn on top of each other must fail the
/// overlap check, a one-line box holding long prose must fail the wrap check, and a clean column
/// must pass.
#[gpui::test]
fn the_probe_fails_overlap_and_clipping_and_passes_a_clean_column(cx: &mut gpui::TestAppContext) {
    cx.update(moon_ui::init);
    let _locale = crate::test_locale::force("en");
    let overlap = ProbeSpec {
        container: "probe-body",
        no_overlap: vec![vec!["a".into(), "b".into()]],
        ..Default::default()
    };
    let stacked = host(|_, _, _| {
        div()
            .relative()
            .h(px(40.0))
            .child(
                div()
                    .absolute()
                    .w(px(80.0))
                    .h(px(20.0))
                    .debug_selector(|| "a".into()),
            )
            .child(
                div()
                    .absolute()
                    .left(px(40.0))
                    .w(px(80.0))
                    .h(px(20.0))
                    .debug_selector(|| "b".into()),
            )
            .into_any_element()
    });
    assert!(probe_once(cx, (860.0, 620.0), &stacked, &overlap).is_err());

    let clipped_spec = ProbeSpec {
        container: "probe-body",
        wraps: vec![("prose".into(), LONG.into())],
        ..Default::default()
    };
    let clipped = host(|_, _, _| {
        div()
            .w(px(160.0))
            .h(px(14.0))
            .overflow_hidden()
            .debug_selector(|| "prose".into())
            .child(LONG)
            .into_any_element()
    });
    assert!(probe_once(cx, (860.0, 620.0), &clipped, &clipped_spec).is_err());

    let clean_spec = ProbeSpec {
        container: "probe-body",
        wraps: vec![("prose".into(), LONG.into())],
        inside: vec!["prose".into(), "a".into(), "b".into()],
        no_overlap: vec![vec!["a".into(), "b".into(), "prose".into()]],
        last: Some("b".into()),
        ..Default::default()
    };
    let clean = host(|_, _, _| {
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(div().w(px(80.0)).h(px(20.0)).debug_selector(|| "a".into()))
            .child(div().w_full().debug_selector(|| "prose".into()).child(LONG))
            .child(div().w(px(80.0)).h(px(20.0)).debug_selector(|| "b".into()))
            .into_any_element()
    });
    assert_eq!(probe_once(cx, (860.0, 620.0), &clean, &clean_spec), Ok(()));
}

/// The pages whose names make the longest dirty sentence.
const DIRTY: [Tab; 3] = [Tab::Appearance, Tab::Hotkeys, Tab::Security];

/// The page header with the longest title and two actions, and a dirty bar naming three pages,
/// stay inside the body, keep their title and chip apart, and wrap the bar's sentence without
/// cutting it, in every language at the default and the minimum window.
#[gpui::test]
fn page_header_and_dirty_bar_fit_the_body(cx: &mut gpui::TestAppContext) {
    let longest = || {
        Tab::ALL
            .into_iter()
            .max_by_key(|tab| tab.title().chars().count())
            .unwrap_or(Tab::Cores)
    };
    sweep(
        cx,
        host(move |window, cx, _| {
            let narrow = f32::from(window.viewport_size().width) < super::NAV_BREAKPOINT;
            let action = |id: &'static str| {
                div()
                    .debug_selector(move || id.into())
                    .child(
                        moon_ui::MoonButton::new(id)
                            .outline()
                            .label(rust_i18n::t!("settings.copy").to_string())
                            .render(),
                    )
                    .into_any_element()
            };
            let header = page_header(
                PageHeader {
                    tab: longest(),
                    mode: crate::settings::SaveMode::Draft,
                    desc_override: None,
                    actions: vec![action("probe-act-1"), action("probe-act-2")],
                },
                narrow,
                cx,
            );
            let noop: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new(|_, _| {});
            let bar = save_bar(
                SaveBar::Draft {
                    dirty_pages: DIRTY.to_vec(),
                    dirty: true,
                },
                None,
                noop.clone(),
                noop,
                cx,
            );
            div()
                .w_full()
                .flex()
                .flex_col()
                .child(div().debug_selector(|| "probe-head".into()).child(header))
                .children(bar.map(|bar| {
                    div()
                        .debug_selector(|| "probe-bar".into())
                        .child(bar)
                        .into_any_element()
                }))
                .into_any_element()
        }),
        |_| ProbeSpec {
            container: "probe-body",
            inside: vec![
                "probe-head".into(),
                "probe-bar".into(),
                "probe-act-1".into(),
                "probe-act-2".into(),
            ],
            no_overlap: vec![
                vec!["probe-act-1".into(), "probe-act-2".into()],
                vec!["probe-head".into(), "probe-bar".into()],
            ],
            wraps: vec![(
                "settings-save-bar-text".into(),
                crate::settings::frame::dirty_text(&DIRTY),
            )],
            last: Some("probe-bar".into()),
            ..Default::default()
        },
    );
}

/// Three hint rows — a short hint, a long help unfolded, a number field with a unit — keep their
/// controls inside the body and their unfolded help uncut, in every language. The number field
/// must stay at least as wide as its widest value and unit: it used to collapse to an empty box
/// between its buttons.
#[gpui::test]
fn hint_rows_fit_the_body(cx: &mut gpui::TestAppContext) {
    sweep(
        cx,
        |window, cx: &mut Context<Host>, (width, height)| {
            let state = cx.new(|cx| MoonInputState::new(window, cx));
            let base = host(move |_, cx, _| {
                let noop: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new(|_, _| {});
                let field = div()
                    .debug_selector(|| "probe-num".into())
                    .child(num_input(
                        "probe-num-input",
                        &state,
                        0.0..=100000.0,
                        1.0,
                        Some("px".into()),
                    ))
                    .into_any_element();
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .debug_selector(|| "probe-row-1".into())
                            .child(hint_row(
                                "probe-row-1",
                                rust_i18n::t!("settings.page.general.title").to_string(),
                                Some(rust_i18n::t!("settings.page.general.desc").to_string()),
                                None,
                                div().w(px(120.0)).h(px(24.0)).into_any_element(),
                                false,
                                noop.clone(),
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .debug_selector(|| "probe-row-2".into())
                            .child(hint_row(
                                "probe-row-2",
                                rust_i18n::t!("settings.page.security.title").to_string(),
                                Some(rust_i18n::t!("settings.page.security.desc").to_string()),
                                Some(rust_i18n::t!("settings.page.storage.desc").to_string()),
                                div().w(px(120.0)).h(px(24.0)).into_any_element(),
                                true,
                                noop.clone(),
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .debug_selector(|| "probe-row-3".into())
                            .child(hint_row(
                                "probe-row-3",
                                rust_i18n::t!("settings.page.sounds.title").to_string(),
                                None,
                                None,
                                field,
                                false,
                                noop,
                                cx,
                            )),
                    )
                    .into_any_element()
            });
            base(window, cx, (width, height))
        },
        |_| ProbeSpec {
            container: "probe-body",
            inside: vec![
                "probe-row-1".into(),
                "probe-row-2".into(),
                "probe-row-3".into(),
                "probe-num".into(),
            ],
            no_overlap: vec![vec![
                "probe-row-1".into(),
                "probe-row-2".into(),
                "probe-row-3".into(),
            ]],
            fits: vec![
                (
                    "probe-row-3-label".into(),
                    rust_i18n::t!("settings.page.sounds.title").to_string(),
                    TextFace::Ui,
                ),
                ("probe-num".into(), "100000 px".into(), TextFace::Ui),
            ],
            wraps: vec![(
                "probe-row-2-more-text".into(),
                rust_i18n::t!("settings.page.storage.desc").to_string(),
            )],
            last: Some("probe-row-3".into()),
            ..Default::default()
        },
    );
}

/// Catches the page title collapsing to an ellipsis beside its save-mode chip (a truncating text
/// inside a nested flex row shrinks to nothing): every page's header shows its full title, chip
/// and two actions alongside, in every language at the default and the minimum window.
#[gpui::test]
fn every_page_header_shows_its_full_title(cx: &mut gpui::TestAppContext) {
    sweep(
        cx,
        host(move |window, cx, _| {
            let narrow = f32::from(window.viewport_size().width) < super::NAV_BREAKPOINT;
            let action = |id: &'static str| {
                moon_ui::MoonButton::new(id)
                    .outline()
                    .label(rust_i18n::t!("settings.copy").to_string())
                    .render()
                    .into_any_element()
            };
            div()
                .w_full()
                .flex()
                .flex_col()
                .children(Tab::ALL.into_iter().map(|tab| {
                    page_header(
                        PageHeader {
                            tab,
                            mode: crate::settings::SaveMode::Remote,
                            desc_override: None,
                            actions: vec![action("probe-a"), action("probe-b")],
                        },
                        narrow,
                        cx,
                    )
                }))
                .into_any_element()
        }),
        |_| ProbeSpec {
            container: "probe-body",
            fits: Tab::ALL
                .into_iter()
                .map(|tab| {
                    (
                        format!("settings-page-title-{}", tab.id()),
                        tab.title(),
                        TextFace::Title,
                    )
                })
                .collect(),
            ..Default::default()
        },
    );
}
